use rust_decimal::Decimal;
use crate::events::Side;

/// Position state - tracks inventory and cost basis.
/// Updated only after confirmed fills.
#[derive(Debug, Clone, Default)]
pub struct Position {
    /// Quantity of YES shares owned
    pub qty_yes: Decimal,
    /// Quantity of NO shares owned
    pub qty_no: Decimal,
    /// Total cost of YES shares (in ticks)
    pub cost_yes: Decimal,
    /// Total cost of NO shares (in ticks)
    pub cost_no: Decimal,
}

impl Position {
    /// Average price per YES share in ticks.
    /// Returns None if no YES shares owned.
    pub fn avg_price_yes(&self) -> Option<Decimal> {
        if self.qty_yes > Decimal::ZERO {
            Some(self.cost_yes / self.qty_yes)
        } else {
            None
        }
    }

    /// Average price per NO share in ticks.
    /// Returns None if no NO shares owned.
    pub fn avg_price_no(&self) -> Option<Decimal> {
        if self.qty_no > Decimal::ZERO {
            Some(self.cost_no / self.qty_no)
        } else {
            None
        }
    }

    /// Net position: positive = heavy YES, negative = heavy NO.
    pub fn net_position(&self) -> Decimal {
        self.qty_yes - self.qty_no
    }

    /// Absolute imbalance between YES and NO.
    pub fn imbalance(&self) -> Decimal {
        (self.qty_yes - self.qty_no).abs()
    }

    /// Total cost of a complete pair (YES + NO) in ticks.
    /// Returns None if missing one side.
    pub fn pair_cost(&self) -> Option<Decimal> {
        match (self.avg_price_yes(), self.avg_price_no()) {
            (Some(y), Some(n)) => Some(y + n),
            _ => None,
        }
    }

    /// Minimum guaranteed P&L in ticks.
    /// min(qty) shares will redeem at 1000 ticks ($1.00).
    pub fn min_pnl_ticks(&self) -> Decimal {
        let min_qty = self.qty_yes.min(self.qty_no);
        let payout = min_qty * Decimal::from(1000);
        let total_cost = self.cost_yes + self.cost_no;
        payout - total_cost
    }

    /// Minimum guaranteed P&L in dollars.
    pub fn min_pnl_usd(&self) -> Decimal {
        self.min_pnl_ticks() / Decimal::from(1000)
    }

    /// Apply a fill to the position.
    pub fn apply_fill(&mut self, side: Side, price_ticks: u16, size: Decimal) {
        let cost = Decimal::from(price_ticks) * size;
        match side {
            Side::Yes => {
                self.qty_yes += size;
                self.cost_yes += cost;
            }
            Side::No => {
                self.qty_no += size;
                self.cost_no += cost;
            }
        }
    }

    /// Get quantity for a side.
    pub fn qty(&self, side: Side) -> Decimal {
        match side {
            Side::Yes => self.qty_yes,
            Side::No => self.qty_no,
        }
    }

    /// Check if position is empty.
    pub fn is_empty(&self) -> bool {
        self.qty_yes == Decimal::ZERO && self.qty_no == Decimal::ZERO
    }

    /// Check if holding both sides.
    pub fn has_both_sides(&self) -> bool {
        self.qty_yes > Decimal::ZERO && self.qty_no > Decimal::ZERO
    }

    /// Reset position (e.g., on market switch).
    pub fn reset(&mut self) {
        self.qty_yes = Decimal::ZERO;
        self.qty_no = Decimal::ZERO;
        self.cost_yes = Decimal::ZERO;
        self.cost_no = Decimal::ZERO;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_empty_position() {
        let pos = Position::default();
        assert!(pos.is_empty());
        assert_eq!(pos.net_position(), Decimal::ZERO);
        assert_eq!(pos.imbalance(), Decimal::ZERO);
        assert_eq!(pos.pair_cost(), None);
    }

    #[test]
    fn test_apply_fill() {
        let mut pos = Position::default();

        // Buy 10 YES at 450 ticks (45c)
        pos.apply_fill(Side::Yes, 450, dec!(10));
        assert_eq!(pos.qty_yes, dec!(10));
        assert_eq!(pos.cost_yes, dec!(4500)); // 10 * 450
        assert_eq!(pos.avg_price_yes(), Some(dec!(450)));

        // Buy 10 NO at 520 ticks (52c)
        pos.apply_fill(Side::No, 520, dec!(10));
        assert_eq!(pos.qty_no, dec!(10));
        assert_eq!(pos.cost_no, dec!(5200)); // 10 * 520

        // Pair cost = 450 + 520 = 970 ticks (97c) - profitable!
        assert_eq!(pos.pair_cost(), Some(dec!(970)));
    }

    #[test]
    fn test_net_position_and_imbalance() {
        let mut pos = Position::default();

        pos.apply_fill(Side::Yes, 500, dec!(30));
        pos.apply_fill(Side::No, 500, dec!(20));

        // Heavy YES by 10
        assert_eq!(pos.net_position(), dec!(10));
        assert_eq!(pos.imbalance(), dec!(10));
    }

    #[test]
    fn test_min_pnl() {
        let mut pos = Position::default();

        // 10 YES at 450, 10 NO at 520 = pair cost 970
        pos.apply_fill(Side::Yes, 450, dec!(10));
        pos.apply_fill(Side::No, 520, dec!(10));

        // min(10, 10) = 10 pairs redeem at 1000 each = 10000
        // total cost = 4500 + 5200 = 9700
        // profit = 10000 - 9700 = 300 ticks = $0.30
        assert_eq!(pos.min_pnl_ticks(), dec!(300));
        assert_eq!(pos.min_pnl_usd(), dec!(0.3));
    }

    #[test]
    fn test_imbalanced_pnl() {
        let mut pos = Position::default();

        // 20 YES at 450, 10 NO at 520
        pos.apply_fill(Side::Yes, 450, dec!(20));
        pos.apply_fill(Side::No, 520, dec!(10));

        // Only 10 pairs can redeem
        // payout = 10 * 1000 = 10000
        // cost = 20*450 + 10*520 = 9000 + 5200 = 14200
        // min_pnl = 10000 - 14200 = -4200 ticks = -$4.20
        assert_eq!(pos.min_pnl_ticks(), dec!(-4200));
        assert_eq!(pos.min_pnl_usd(), dec!(-4.2));
    }
}
