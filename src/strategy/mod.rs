mod actions;
mod btc_guard;
mod flow;
mod pricing;
mod sizing;
mod variance;

pub use actions::Action;
pub use btc_guard::{BtcGuard, BtcGuardConfig};
pub use flow::FlowEstimator;
pub use pricing::{calc_max_bid, AvellanedaStoikov, Quotes, P_MAX, P_MIN};
pub use sizing::{calc_size, calc_size_with_limit, can_place, MarketDuration};
pub use variance::VarianceEstimator;

use crate::events::Side;
use crate::state::{OrderTracker, Position};
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
/// # Arguments
/// * `quotes` - A-S computed quotes (yes_bid, no_bid in probability space)
/// * `p_mid` - Current mid probability (for should_quote check)
/// * `position` - Current inventory
/// * `orders` - Current standing orders
/// * `time_remaining` - Seconds until market settlement
/// * `config` - Strategy configuration
///
/// # Returns
/// Vec of actions to execute (Place, Cancel, CancelAll)
pub fn reconcile(
    quotes: &Quotes,
    p_mid: f64,
    position: &Position,
    orders: &OrderTracker,
    time_remaining: f64,
    config: &StrategyConfig,
) -> Vec<Action> {
    let mut actions = Vec::new();

    // Check if we should quote at all
    if !Quotes::should_quote(p_mid) {
        // Outside valid range - cancel all orders
        if orders.total_count() > 0 {
            actions.push(Action::CancelAll);
        }
        return actions;
    }

    // Convert A-S quotes to ticks
    let yes_top_tick = AvellanedaStoikov::to_ticks(quotes.yes_bid);
    let no_top_tick = AvellanedaStoikov::to_ticks(quotes.no_bid);

    // Calculate size for each side
    let time_remaining_secs = time_remaining as i64;
    let yes_size = calc_size_with_limit(
        Side::Yes,
        position,
        time_remaining_secs,
        config.duration,
        config.max_position,
    );
    let no_size = calc_size_with_limit(
        Side::No,
        position,
        time_remaining_secs,
        config.duration,
        config.max_position,
    );

    // Build ideal ladders
    let yes_ideal = build_ladder(yes_top_tick, yes_size, config);
    let no_ideal = build_ladder(no_top_tick, no_size, config);

    // Reconcile YES side
    reconcile_side(Side::Yes, &yes_ideal, orders, config, &mut actions);

    // Reconcile NO side
    reconcile_side(Side::No, &no_ideal, orders, config, &mut actions);

    actions
}

/// Reconcile a single side: cancel stale orders, place missing orders.
fn reconcile_side(
    side: Side,
    ideal: &HashMap<u16, Decimal>,
    orders: &OrderTracker,
    config: &StrategyConfig,
    actions: &mut Vec<Action>,
) {
    // 1. Cancel orders at prices not in ideal ladder
    for price in orders.prices(side) {
        if !ideal.contains_key(&price) {
            for order in orders.orders_at_price(side, price) {
                actions.push(Action::Cancel {
                    order_id: order.order_id.clone(),
                });
            }
        }
    }

    // 2. Place orders at ideal prices where we're short
    for (&price, &target_size) in ideal {
        let current_size = orders.total_size_at_price(side, price);
        if current_size < target_size {
            let needed = target_size - current_size;
            if needed >= config.min_order_size {
                actions.push(Action::Place {
                    side,
                    price,
                    size: needed,
                });
            }
        }
    }
}

/// Build ideal ladder: {price: size, price-spacing: size, ...}
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
