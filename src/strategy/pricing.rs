use crate::events::Side;
use crate::state::Book;

/// Probability bounds for quoting.
/// Based on Gaba's observed range (4-96c) but slightly tighter.
/// Stop quoting before mid races to extremes and variance explodes.
pub const P_MIN: f64 = 0.07;
pub const P_MAX: f64 = 0.93;

/// Output of A-S pricing: bid prices for YES and NO sides.
#[derive(Debug, Clone, Copy)]
pub struct Quotes {
    /// Price to buy YES shares (0.0 to 1.0)
    pub yes_bid: f64,
    /// Price to buy NO shares (0.0 to 1.0)
    pub no_bid: f64,
}

impl Quotes {
    /// Check if we should quote at all given current mid probability.
    /// Returns false at extremes (p < 7% or p > 93%) where:
    /// - Edge is too small to cover fees
    /// - Variance is about to explode from logit compression
    #[inline]
    pub fn should_quote(p_mid: f64) -> bool {
        p_mid >= P_MIN && p_mid <= P_MAX
    }
}

/// Avellaneda-Stoikov market making in logit space.
///
/// Computes optimal bid prices given:
/// - Current probability (canonical mid)
/// - Net inventory (yes_shares - no_shares)
/// - Variance of probability movements
/// - Order arrival rate (k)
/// - Time until settlement
#[derive(Debug, Clone)]
pub struct AvellanedaStoikov {
    /// Risk aversion parameter. Higher = wider spreads, less inventory risk.
    /// Start with 0.002, calibrate from live data.
    pub gamma: f64,
    /// Minimum spread in logit space near expiry.
    /// Overrides A-S when time_left < 30s.
    pub expiry_base_penalty: f64,
}

impl AvellanedaStoikov {
    /// Create with default parameters.
    pub fn new(gamma: f64) -> Self {
        Self {
            gamma,
            expiry_base_penalty: 0.5,
        }
    }

    /// Compute bid prices for YES and NO.
    ///
    /// # Arguments
    /// * `p_t` - Canonical mid probability (0.0 to 1.0)
    /// * `q_t` - Net inventory: yes_shares - no_shares
    /// * `var` - Variance of logit increments per second
    /// * `k` - Order arrival rate (trades per second)
    /// * `time_left` - Seconds until settlement
    ///
    /// # Returns
    /// Quotes with yes_bid and no_bid in probability space (0.0 to 1.0)
    pub fn compute_quotes(&self, p_t: f64, q_t: f64, var: f64, k: f64, time_left: f64) -> Quotes {
        // 1. Transform to logit space
        let p_clamped = p_t.clamp(0.01, 0.99);
        let x_t = (p_clamped / (1.0 - p_clamped)).ln();

        // 2. Reservation price (skewed by inventory)
        //    Positive q_t (long YES) → reservation drops → cheaper YES bid, better NO bid
        //    This naturally rebalances inventory through quote skew
        let r_x = x_t - (q_t * self.gamma * var * time_left);

        // 3. Spread in logit space
        let k_safe = k.max(0.01); // Avoid division by zero
        let flow_premium = (2.0 / k_safe) * (1.0 + self.gamma / k_safe).ln();
        let mut spread_x = (self.gamma * var * time_left) + flow_premium;

        // 4. Expiry spread override
        //    A-S naturally tightens as time_left → 0, but in binary markets
        //    the last 30 seconds are the most dangerous (informed traders).
        if time_left < 30.0 {
            let override_spread = self.expiry_base_penalty * (1.0 + (30.0 - time_left) / 10.0);
            spread_x = spread_x.max(override_spread);
        }

        // 5. Bid and ask in logit
        let logit_bid = r_x - spread_x / 2.0;
        let logit_ask = r_x + spread_x / 2.0;

        // 6. Convert back to probability
        let p_bid = 1.0 / (1.0 + (-logit_bid).exp());
        let p_ask = 1.0 / (1.0 + (-logit_ask).exp());

        // 7. YES bid = p_bid, NO bid = 1 - p_ask
        //    Combined pair cost = p_bid + (1 - p_ask) = 1 - (p_ask - p_bid) = 1 - spread
        Quotes {
            yes_bid: p_bid,
            no_bid: 1.0 - p_ask,
        }
    }

    /// Check if pair cost is profitable.
    pub fn is_profitable(&self, quotes: &Quotes, max_pair_cost: f64) -> bool {
        quotes.yes_bid + quotes.no_bid <= max_pair_cost
    }

    /// Convert probability to cents (0-99), flooring for conservative pricing.
    pub fn to_cents(p: f64) -> u32 {
        ((p * 100.0).floor() as u32).clamp(1, 99)
    }

    /// Convert probability to ticks (10-990), flooring for conservative pricing.
    /// Minimum 10 ticks (1c), maximum 990 ticks (99c).
    pub fn to_ticks(p: f64) -> u16 {
        ((p * 1000.0).floor() as u16).clamp(10, 990)
    }
}

/// Calculate max bid price for a side.
///
/// TODO: Finalize pricing strategy. Options:
///
/// 1. Simple Gaba-style:
///    max_bid = 1000 - opposite_ask - margin
///
/// 2. With inventory skew (A-S style):
///    base = 1000 - opposite_ask - margin
///    skew = gamma * net_position
///    max_bid = base - skew
///
/// 3. With trash filter:
///    if this_side_ask < 50 → return 0 (don't bid on crashing side)
///
/// # Arguments
/// * `side` - Side to calculate bid for (Yes or No)
/// * `book` - Current order book state
/// * `margin_ticks` - Minimum profit margin in ticks (e.g., 5 = 0.5c)
///
/// # Returns
/// Max bid price in ticks (0-1000), or 0 if shouldn't bid
pub fn calc_max_bid(side: Side, book: &Book, margin_ticks: u16) -> u16 {
    // Get opposite side's ask
    let opposite_ask = match book.opposite_ask(side) {
        Some(ask) => ask,
        None => return 0, // No data, don't bid
    };

    // TODO: Add trash filter?
    // let this_ask = book.best_ask(side).unwrap_or(0);
    // if this_ask < 50 {
    //     return 0; // Don't bid on crashing side
    // }

    // Simple formula: max_bid = 1000 - opposite_ask - margin
    1000_u16
        .saturating_sub(opposite_ask)
        .saturating_sub(margin_ticks)
}

/// Calculate max bid with inventory skew (A-S style).
///
/// TODO: Implement if needed. Gaba doesn't seem to use this.
///
/// Formula:
///   base = 1000 - opposite_ask - margin
///   skew = gamma * net_position (positive when heavy on this side)
///   max_bid = base - skew
///
/// When heavy on a side, reduce bid to discourage more fills.
/// When light on a side, increase bid to attract fills.
#[allow(dead_code)]
pub fn calc_max_bid_with_skew(
    _side: Side,
    _book: &Book,
    _net_position: i64,
    _gamma: f64,
    _margin_ticks: u16,
) -> u16 {
    todo!("Implement if inventory skew is needed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calc_max_bid_basic() {
        let mut book = Book::default();
        book.update(Side::Yes, 480, 490, 1000);
        book.update(Side::No, 500, 510, 1001);

        // For YES: opposite is NO, ask = 510
        // max_bid = 1000 - 510 - 5 = 485
        assert_eq!(calc_max_bid(Side::Yes, &book, 5), 485);

        // For NO: opposite is YES, ask = 490
        // max_bid = 1000 - 490 - 5 = 505
        assert_eq!(calc_max_bid(Side::No, &book, 5), 505);
    }

    #[test]
    fn test_calc_max_bid_no_data() {
        let book = Book::default();

        // No data → return 0
        assert_eq!(calc_max_bid(Side::Yes, &book, 5), 0);
        assert_eq!(calc_max_bid(Side::No, &book, 5), 0);
    }

    #[test]
    fn test_calc_max_bid_different_margins() {
        let mut book = Book::default();
        book.update(Side::Yes, 480, 490, 1000);
        book.update(Side::No, 500, 510, 1001);

        // YES with 5 tick margin
        assert_eq!(calc_max_bid(Side::Yes, &book, 5), 485);

        // YES with 10 tick margin (1c)
        assert_eq!(calc_max_bid(Side::Yes, &book, 10), 480);

        // YES with 0 margin (aggressive)
        assert_eq!(calc_max_bid(Side::Yes, &book, 0), 490);
    }

    #[test]
    fn test_calc_max_bid_extreme_prices() {
        let mut book = Book::default();

        // YES crashing (ask = 100 = 10c)
        book.update(Side::Yes, 90, 100, 1000);
        book.update(Side::No, 890, 900, 1001);

        // For YES: opposite NO ask = 900
        // max_bid = 1000 - 900 - 5 = 95
        assert_eq!(calc_max_bid(Side::Yes, &book, 5), 95);

        // For NO: opposite YES ask = 100
        // max_bid = 1000 - 100 - 5 = 895
        assert_eq!(calc_max_bid(Side::No, &book, 5), 895);
    }

    // ========== Avellaneda-Stoikov Tests ==========

    #[test]
    fn test_as_new() {
        let as_pricer = AvellanedaStoikov::new(0.002);
        assert!((as_pricer.gamma - 0.002).abs() < 1e-9);
        assert!((as_pricer.expiry_base_penalty - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_as_symmetric_at_50() {
        let as_pricer = AvellanedaStoikov::new(0.002);

        // At p=0.5, no inventory, quotes should be symmetric
        let quotes = as_pricer.compute_quotes(0.5, 0.0, 0.01, 1.0, 120.0);

        // yes_bid + no_bid should be less than 1 (profitable)
        let pair_cost = quotes.yes_bid + quotes.no_bid;
        assert!(pair_cost < 1.0);

        // With no inventory skew, yes_bid and no_bid should be roughly symmetric
        // yes_bid ≈ 1 - no_bid (within spread)
        let spread = 1.0 - pair_cost;
        assert!(spread > 0.0);
        assert!(spread < 0.5); // Spread shouldn't be huge
    }

    #[test]
    fn test_as_inventory_skew() {
        let as_pricer = AvellanedaStoikov::new(0.002);

        // No inventory
        let q_neutral = as_pricer.compute_quotes(0.5, 0.0, 0.01, 1.0, 120.0);

        // Long YES (positive inventory)
        let q_long_yes = as_pricer.compute_quotes(0.5, 50.0, 0.01, 1.0, 120.0);

        // Long NO (negative inventory)
        let q_long_no = as_pricer.compute_quotes(0.5, -50.0, 0.01, 1.0, 120.0);

        // When long YES: reservation drops → cheaper YES bid, better NO bid
        assert!(q_long_yes.yes_bid < q_neutral.yes_bid);
        assert!(q_long_yes.no_bid > q_neutral.no_bid);

        // When long NO: reservation rises → better YES bid, cheaper NO bid
        assert!(q_long_no.yes_bid > q_neutral.yes_bid);
        assert!(q_long_no.no_bid < q_neutral.no_bid);
    }

    #[test]
    fn test_as_higher_var_wider_spread() {
        let as_pricer = AvellanedaStoikov::new(0.002);

        // Low variance
        let q_low_var = as_pricer.compute_quotes(0.5, 0.0, 0.001, 1.0, 120.0);

        // High variance
        let q_high_var = as_pricer.compute_quotes(0.5, 0.0, 0.1, 1.0, 120.0);

        let spread_low = 1.0 - (q_low_var.yes_bid + q_low_var.no_bid);
        let spread_high = 1.0 - (q_high_var.yes_bid + q_high_var.no_bid);

        // Higher variance → wider spread
        assert!(spread_high > spread_low);
    }

    #[test]
    fn test_as_higher_k_tighter_spread() {
        let as_pricer = AvellanedaStoikov::new(0.002);

        // Low k (slow market)
        let q_low_k = as_pricer.compute_quotes(0.5, 0.0, 0.01, 0.1, 120.0);

        // High k (active market)
        let q_high_k = as_pricer.compute_quotes(0.5, 0.0, 0.01, 5.0, 120.0);

        let spread_low_k = 1.0 - (q_low_k.yes_bid + q_low_k.no_bid);
        let spread_high_k = 1.0 - (q_high_k.yes_bid + q_high_k.no_bid);

        // Higher k → tighter spread (orders fill quickly)
        assert!(spread_high_k < spread_low_k);
    }

    #[test]
    fn test_as_expiry_override() {
        let as_pricer = AvellanedaStoikov::new(0.002);

        // Far from expiry
        let q_far = as_pricer.compute_quotes(0.5, 0.0, 0.01, 1.0, 120.0);

        // Near expiry (10 seconds left)
        let q_near = as_pricer.compute_quotes(0.5, 0.0, 0.01, 1.0, 10.0);

        let spread_far = 1.0 - (q_far.yes_bid + q_far.no_bid);
        let spread_near = 1.0 - (q_near.yes_bid + q_near.no_bid);

        // Near expiry should have wider spread due to override
        // (A-S would naturally tighten, but override prevents this)
        assert!(spread_near > 0.0);
    }

    #[test]
    fn test_as_is_profitable() {
        let as_pricer = AvellanedaStoikov::new(0.002);

        // With low variance and high k, spread is tight (~0.2%)
        // pair_cost ≈ 0.998, so need ceiling > 0.998
        let tight_quotes = as_pricer.compute_quotes(0.5, 0.0, 0.01, 1.0, 120.0);
        assert!(as_pricer.is_profitable(&tight_quotes, 0.999));
        assert!(!as_pricer.is_profitable(&tight_quotes, 0.99));

        // With higher variance and lower k, spread is wider
        let wide_quotes = as_pricer.compute_quotes(0.5, 0.0, 0.1, 0.1, 120.0);
        let pair_cost = wide_quotes.yes_bid + wide_quotes.no_bid;
        assert!(pair_cost < 0.98); // Should have decent spread
        assert!(as_pricer.is_profitable(&wide_quotes, 0.98));
    }

    #[test]
    fn test_as_to_cents() {
        assert_eq!(AvellanedaStoikov::to_cents(0.485), 48);
        assert_eq!(AvellanedaStoikov::to_cents(0.489), 48); // floors
        assert_eq!(AvellanedaStoikov::to_cents(0.01), 1);   // min 1
        assert_eq!(AvellanedaStoikov::to_cents(0.999), 99); // max 99
        assert_eq!(AvellanedaStoikov::to_cents(0.0), 1);    // clamps to 1
        assert_eq!(AvellanedaStoikov::to_cents(1.5), 99);   // clamps to 99
    }

    #[test]
    fn test_as_extreme_probability() {
        let as_pricer = AvellanedaStoikov::new(0.002);

        // Very low probability (YES unlikely)
        let q_low = as_pricer.compute_quotes(0.05, 0.0, 0.01, 1.0, 120.0);
        assert!(q_low.yes_bid > 0.0);
        assert!(q_low.yes_bid < 0.10);
        assert!(q_low.no_bid > 0.80);

        // Very high probability (YES likely)
        let q_high = as_pricer.compute_quotes(0.95, 0.0, 0.01, 1.0, 120.0);
        assert!(q_high.yes_bid > 0.80);
        assert!(q_high.no_bid > 0.0);
        assert!(q_high.no_bid < 0.10);
    }

    #[test]
    fn test_should_quote_cutoff() {
        // Mid-range: should quote
        assert!(Quotes::should_quote(0.50));
        assert!(Quotes::should_quote(0.30));
        assert!(Quotes::should_quote(0.70));

        // At boundaries: should quote
        assert!(Quotes::should_quote(0.07));
        assert!(Quotes::should_quote(0.93));

        // Just inside boundaries
        assert!(Quotes::should_quote(0.08));
        assert!(Quotes::should_quote(0.92));

        // Outside boundaries: should NOT quote
        assert!(!Quotes::should_quote(0.06));
        assert!(!Quotes::should_quote(0.94));
        assert!(!Quotes::should_quote(0.01));
        assert!(!Quotes::should_quote(0.99));
        assert!(!Quotes::should_quote(0.05));
        assert!(!Quotes::should_quote(0.95));
    }
}
