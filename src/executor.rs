//! Executor: turns Actions into API calls via polyfill_rs.
//!
//! Handles order placement, cancellation, and OrderTracker updates.

use anyhow::Result;
use polyfill_rs::{ClobClient, OrderArgs, OrderType, Side as PolySide};
use polyfill_rs::types::ExtraOrderArgs;
use alloy_primitives::U256;
use rust_decimal::Decimal;
use tracing::{info, warn, error};

use crate::events::Side;
use crate::state::OrderTracker;
use crate::strategy::Action;

/// Configuration for the executor.
pub struct ExecutorConfig {
    /// If true, log actions instead of executing them.
    pub log_only: bool,
    /// YES token ID for this market.
    pub yes_token: String,
    /// NO token ID for this market.
    pub no_token: String,
}

/// Executes actions via the Polymarket CLOB API.
pub struct Executor {
    client: ClobClient,
    config: ExecutorConfig,
}

impl Executor {
    /// Create a new executor with the given client and config.
    pub fn new(client: ClobClient, config: ExecutorConfig) -> Self {
        Self { client, config }
    }

    /// Update market tokens (on market switch).
    pub fn set_market(&mut self, yes_token: String, no_token: String) {
        self.config.yes_token = yes_token;
        self.config.no_token = no_token;
    }

    /// Execute a list of actions, updating the order tracker.
    pub async fn execute(&self, actions: Vec<Action>, orders: &mut OrderTracker) -> Result<()> {
        if actions.is_empty() {
            return Ok(());
        }

        if self.config.log_only {
            for action in &actions {
                info!("[DRY RUN] {:?}", action);
            }
            return Ok(());
        }

        // Separate actions by type for batching
        let mut places: Vec<&Action> = Vec::new();
        let mut cancels: Vec<&Action> = Vec::new();
        let mut cancel_all = false;
        let mut takes: Vec<&Action> = Vec::new();

        for action in &actions {
            match action {
                Action::Place { .. } => places.push(action),
                Action::Cancel { .. } => cancels.push(action),
                Action::CancelAll => cancel_all = true,
                Action::Take { .. } => takes.push(action),
            }
        }

        // Execute CancelAll first (clears everything)
        if cancel_all {
            self.execute_cancel_all(orders).await?;
        }

        // Execute individual cancels
        for action in cancels {
            if let Action::Cancel { order_id } = action {
                self.execute_cancel(order_id, orders).await?;
            }
        }

        // Execute places (could batch these, but start simple)
        for action in places {
            if let Action::Place { side, price, size } = action {
                self.execute_place(*side, *price, *size, orders).await?;
            }
        }

        // Execute takes (IOC orders for rebalancing)
        for action in takes {
            if let Action::Take { side, size, max_price } = action {
                self.execute_take(*side, *size, *max_price).await?;
            }
        }

        Ok(())
    }

    /// Place a limit order.
    async fn execute_place(
        &self,
        side: Side,
        price: u16,
        size: Decimal,
        orders: &mut OrderTracker,
    ) -> Result<()> {
        let token_id = self.token_for_side(side);

        // Convert price from ticks (0-1000) to decimal (0.001-1.000)
        let price_dec = Decimal::new(price as i64, 3);

        let args = OrderArgs::new(token_id, price_dec, size, PolySide::BUY);
        let extras = ExtraOrderArgs {
            fee_rate_bps: 1000,
            nonce: U256::ZERO,
            taker: "0x0000000000000000000000000000000000000000".to_string(),
        };

        match self.client.create_order(&args, None, Some(extras), None).await {
            Ok(order) => {
                match self.client.post_order(order, OrderType::GTC).await {
                    Ok(response) => {
                        info!(
                            "Placed {:?} {} @ {} ticks → {}",
                            side, size, price, &response.order_id[..20.min(response.order_id.len())]
                        );
                        orders.add(side, response.order_id, price, size);
                    }
                    Err(e) => {
                        error!("Failed to post order: {}", e);
                    }
                }
            }
            Err(e) => {
                error!("Failed to create order: {}", e);
            }
        }

        Ok(())
    }

    /// Cancel a specific order.
    async fn execute_cancel(&self, order_id: &str, orders: &mut OrderTracker) -> Result<()> {
        match self.client.cancel(order_id).await {
            Ok(_) => {
                info!("Cancelled order {}", &order_id[..20.min(order_id.len())]);
                // Try to remove from both sides (we may not know which side)
                if orders.remove_by_id(Side::Yes, order_id).is_none() {
                    orders.remove_by_id(Side::No, order_id);
                }
            }
            Err(e) => {
                warn!("Failed to cancel order {}: {}", &order_id[..20.min(order_id.len())], e);
            }
        }
        Ok(())
    }

    /// Cancel all orders.
    async fn execute_cancel_all(&self, orders: &mut OrderTracker) -> Result<()> {
        // Cancel YES orders
        for order_id in orders.all_order_ids(Side::Yes) {
            if let Err(e) = self.client.cancel(order_id).await {
                warn!("Failed to cancel {}: {}", &order_id[..20.min(order_id.len())], e);
            }
        }

        // Cancel NO orders
        for order_id in orders.all_order_ids(Side::No) {
            if let Err(e) = self.client.cancel(order_id).await {
                warn!("Failed to cancel {}: {}", &order_id[..20.min(order_id.len())], e);
            }
        }

        orders.clear_all();
        info!("Cancelled all orders");
        Ok(())
    }

    /// Execute a taker order (IOC) for rebalancing.
    async fn execute_take(&self, side: Side, size: Decimal, max_price: u16) -> Result<()> {
        let token_id = self.token_for_side(side);

        // Convert price from ticks to decimal
        let price_dec = Decimal::new(max_price as i64, 3);

        let args = OrderArgs::new(token_id, price_dec, size, PolySide::BUY);
        let extras = ExtraOrderArgs {
            fee_rate_bps: 1000,
            nonce: U256::ZERO,
            taker: "0x0000000000000000000000000000000000000000".to_string(),
        };

        match self.client.create_order(&args, None, Some(extras), None).await {
            Ok(order) => {
                // Use FOK (Fill or Kill) for taker orders
                match self.client.post_order(order, OrderType::FOK).await {
                    Ok(response) => {
                        info!(
                            "Take {:?} {} @ max {} ticks → {}",
                            side, size, max_price, &response.order_id[..20.min(response.order_id.len())]
                        );
                        // Note: Don't add to OrderTracker - FOK fills immediately or fails
                    }
                    Err(e) => {
                        warn!("Take order failed (may not have filled): {}", e);
                    }
                }
            }
            Err(e) => {
                error!("Failed to create take order: {}", e);
            }
        }

        Ok(())
    }

    /// Get token ID for a side.
    fn token_for_side(&self, side: Side) -> &str {
        match side {
            Side::Yes => &self.config.yes_token,
            Side::No => &self.config.no_token,
        }
    }
}
