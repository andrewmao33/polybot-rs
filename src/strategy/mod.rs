mod actions;
mod pricing;
mod sizing;

pub use actions::Action;
pub use pricing::calc_max_bid;
pub use sizing::{calc_size, calc_size_with_limit, can_place, MarketDuration};

use crate::events::Side;
use crate::state::{Book, Market, OrderTracker, Position};
use rust_decimal::Decimal;
use std::collections::HashMap;

/// Strategy configuration.
#[derive(Debug, Clone)]
pub struct StrategyConfig {
    /// Minimum profit margin in ticks (e.g., 5 = 0.5c)
    pub margin_ticks: u16,
    /// Maximum net position per side
    pub max_position: Decimal,
    /// Minimum order size (API limit is 5)
    pub min_order_size: Decimal,
    /// Number of price levels in the ladder
    pub ladder_rungs: u16,
    /// Spacing between ladder rungs in ticks (10 = 1c)
    pub rung_spacing: u16,
    /// Market duration (5m or 15m)
    pub duration: MarketDuration,
    /// Imbalance threshold before rebalancing (shares)
    pub rebalance_threshold: Decimal,
    /// Maximum size to take when rebalancing
    pub max_take_size: Decimal,
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            margin_ticks: 5,                             // 0.5c
            max_position: Decimal::from(150),            // 150 shares
            min_order_size: Decimal::from(5),            // API minimum
            ladder_rungs: 3,                             // 3 price levels
            rung_spacing: 10,                            // 1c spacing
            duration: MarketDuration::FiveMin,           // Default to 5m
            rebalance_threshold: Decimal::from(30),      // 30 share imbalance
            max_take_size: Decimal::from(12),            // Max 12 shares per take
        }
    }
}

/// Main strategy entry point.
///
/// Compares ideal ladder to current orders and returns actions to reconcile.
///
/// TODO: Implement full reconciliation logic. Current stub just returns empty.
///
/// # Flow
/// 1. For each side (YES, NO):
///    a. Calculate max_bid using pricing::calc_max_bid
///    b. Calculate size using sizing::calc_size_with_limit
///    c. Build ideal ladder (price → size)
///    d. Compare to OrderTracker
///    e. Generate Cancel actions for stale orders
///    f. Generate Place actions for missing orders
/// 2. Check rebalance condition
/// 3. Return all actions
pub fn reconcile(
    _book: &Book,
    _position: &Position,
    _orders: &OrderTracker,
    _market: &Market,
    _config: &StrategyConfig,
) -> Vec<Action> {
    // TODO: Implement reconciliation logic
    //
    // Pseudocode:
    //
    // let mut actions = Vec::new();
    // let time_remaining = market.time_remaining_secs(now_ms);
    //
    // for side in [Side::Yes, Side::No] {
    //     // 1. Calculate ideal
    //     let max_bid = calc_max_bid(side, book, config.margin_ticks);
    //     let size = calc_size_with_limit(side, position, time_remaining, config.duration, config.max_position);
    //
    //     // 2. Build ideal ladder
    //     let ideal = build_ladder(max_bid, size, config);
    //
    //     // 3. Cancel stale
    //     for price in orders.prices(side) {
    //         if !ideal.contains_key(&price) {
    //             for order in orders.orders_at_price(side, price) {
    //                 actions.push(Action::cancel(order.order_id.clone()));
    //             }
    //         }
    //     }
    //
    //     // 4. Place missing
    //     for (price, target_size) in &ideal {
    //         let current = orders.total_size_at_price(side, *price);
    //         if current < *target_size {
    //             let diff = *target_size - current;
    //             if diff >= config.min_order_size {
    //                 actions.push(Action::place(side, *price, diff));
    //             }
    //         }
    //     }
    // }
    //
    // // 5. Check rebalance
    // if let Some(take_action) = check_rebalance(position, book, config) {
    //     actions.push(take_action);
    // }
    //
    // actions

    Vec::new()
}

/// Build ideal ladder: {price: size, price-spacing: size, ...}
///
/// TODO: Finalize ladder logic.
#[allow(dead_code)]
fn build_ladder(
    top_price: u16,
    size: Decimal,
    config: &StrategyConfig,
) -> HashMap<u16, Decimal> {
    let mut ladder = HashMap::new();

    if top_price == 0 || size == Decimal::ZERO {
        return ladder; // Empty ladder
    }

    for i in 0..config.ladder_rungs {
        let price = top_price.saturating_sub(i * config.rung_spacing);
        if price >= 100 {
            // Min 10c
            ladder.insert(price, size);
        }
    }

    ladder
}

/// Check if rebalancing is needed and return Take action.
///
/// TODO: Finalize rebalance logic.
#[allow(dead_code)]
fn check_rebalance(
    _position: &Position,
    _book: &Book,
    _config: &StrategyConfig,
) -> Option<Action> {
    // TODO: Implement rebalancing
    //
    // let imbalance = position.imbalance();
    // if imbalance <= config.rebalance_threshold {
    //     return None;
    // }
    //
    // let light_side = if position.net_position() > Decimal::ZERO {
    //     Side::No  // Heavy YES, need NO
    // } else {
    //     Side::Yes // Heavy NO, need YES
    // };
    //
    // let take_size = (imbalance / dec!(3)).min(config.max_take_size);
    //
    // let max_price = book.best_ask(light_side)?;
    // if max_price > 600 {  // Don't overpay
    //     return None;
    // }
    //
    // Some(Action::take(light_side, take_size, max_price))

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_build_ladder_basic() {
        let config = StrategyConfig {
            ladder_rungs: 3,
            rung_spacing: 10,
            ..Default::default()
        };

        let ladder = build_ladder(480, dec!(12), &config);

        assert_eq!(ladder.len(), 3);
        assert_eq!(ladder.get(&480), Some(&dec!(12)));
        assert_eq!(ladder.get(&470), Some(&dec!(12)));
        assert_eq!(ladder.get(&460), Some(&dec!(12)));
    }

    #[test]
    fn test_build_ladder_empty_on_zero() {
        let config = StrategyConfig::default();

        // Zero price
        let ladder = build_ladder(0, dec!(12), &config);
        assert!(ladder.is_empty());

        // Zero size
        let ladder = build_ladder(480, Decimal::ZERO, &config);
        assert!(ladder.is_empty());
    }

    #[test]
    fn test_build_ladder_respects_min_price() {
        let config = StrategyConfig {
            ladder_rungs: 5,
            rung_spacing: 10,
            ..Default::default()
        };

        // Top price at 120, rungs would be 120, 110, 100, 90, 80
        // But 90 and 80 are below min (100), so only 3 rungs
        let ladder = build_ladder(120, dec!(12), &config);

        assert_eq!(ladder.len(), 3);
        assert!(ladder.contains_key(&120));
        assert!(ladder.contains_key(&110));
        assert!(ladder.contains_key(&100));
        assert!(!ladder.contains_key(&90));
    }

    #[test]
    fn test_strategy_config_default() {
        let config = StrategyConfig::default();

        assert_eq!(config.margin_ticks, 5);
        assert_eq!(config.ladder_rungs, 3);
        assert_eq!(config.rung_spacing, 10);
        assert_eq!(config.duration, MarketDuration::FiveMin);
    }
}
