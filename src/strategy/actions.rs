use crate::events::Side;
use rust_decimal::Decimal;

/// Actions that the strategy can request.
/// The executor turns these into API calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Place a limit order (maker).
    /// Will sit in the book until filled or cancelled.
    Place {
        side: Side,
        /// Price in ticks (0-1000, where 1000 = $1.00)
        price: u16,
        /// Size in shares
        size: Decimal,
    },

    /// Cancel an existing order by ID.
    Cancel {
        order_id: String,
    },

    /// Cancel all orders (both sides).
    /// Used for circuit breakers, market switches, etc.
    CancelAll,

    /// Cross the spread to buy immediately (taker).
    /// Used for rebalancing when imbalanced.
    Take {
        side: Side,
        /// Size in shares to buy
        size: Decimal,
        /// Maximum price to pay (won't fill above this)
        max_price: u16,
    },
}

impl Action {
    /// Create a Place action.
    pub fn place(side: Side, price: u16, size: Decimal) -> Self {
        Self::Place { side, price, size }
    }

    /// Create a Cancel action.
    pub fn cancel(order_id: impl Into<String>) -> Self {
        Self::Cancel {
            order_id: order_id.into(),
        }
    }

    /// Create a CancelAll action.
    pub fn cancel_all() -> Self {
        Self::CancelAll
    }

    /// Create a Take action for rebalancing.
    pub fn take(side: Side, size: Decimal, max_price: u16) -> Self {
        Self::Take {
            side,
            size,
            max_price,
        }
    }

    /// Check if this is a Place action.
    pub fn is_place(&self) -> bool {
        matches!(self, Self::Place { .. })
    }

    /// Check if this is a Cancel action.
    pub fn is_cancel(&self) -> bool {
        matches!(self, Self::Cancel { .. } | Self::CancelAll)
    }

    /// Check if this is a Take action.
    pub fn is_take(&self) -> bool {
        matches!(self, Self::Take { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_place_action() {
        let action = Action::place(Side::Yes, 450, dec!(12));
        assert!(action.is_place());
        assert!(!action.is_cancel());
        assert!(!action.is_take());

        if let Action::Place { side, price, size } = action {
            assert_eq!(side, Side::Yes);
            assert_eq!(price, 450);
            assert_eq!(size, dec!(12));
        } else {
            panic!("Expected Place action");
        }
    }

    #[test]
    fn test_cancel_action() {
        let action = Action::cancel("order123");
        assert!(action.is_cancel());

        if let Action::Cancel { order_id } = action {
            assert_eq!(order_id, "order123");
        } else {
            panic!("Expected Cancel action");
        }
    }

    #[test]
    fn test_cancel_all_action() {
        let action = Action::cancel_all();
        assert!(action.is_cancel());
        assert!(matches!(action, Action::CancelAll));
    }

    #[test]
    fn test_take_action() {
        let action = Action::take(Side::No, dec!(10), 550);
        assert!(action.is_take());

        if let Action::Take {
            side,
            size,
            max_price,
        } = action
        {
            assert_eq!(side, Side::No);
            assert_eq!(size, dec!(10));
            assert_eq!(max_price, 550);
        } else {
            panic!("Expected Take action");
        }
    }
}
