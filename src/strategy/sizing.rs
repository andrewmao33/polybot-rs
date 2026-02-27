use crate::events::Side;
use crate::state::Position;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

/// Market duration types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarketDuration {
    FiveMin,
    FifteenMin,
}

impl MarketDuration {
    /// Total duration in seconds.
    pub fn total_secs(&self) -> i64 {
        match self {
            Self::FiveMin => 300,
            Self::FifteenMin => 900,
        }
    }
}

/// Calculate order size based on time remaining.
///
/// Gaba-style sizing: size decreases as market nears end.
/// This is time-based, NOT imbalance-based.
///
/// # Arguments
/// * `time_remaining_secs` - Seconds until market ends
/// * `duration` - Market duration (5m or 15m)
///
/// # Returns
/// Order size in shares (Decimal)
pub fn calc_size(time_remaining_secs: i64, duration: MarketDuration) -> Decimal {
    match duration {
        MarketDuration::FiveMin => calc_size_5m(time_remaining_secs),
        MarketDuration::FifteenMin => calc_size_15m(time_remaining_secs),
    }
}

/// Sizing for 5-minute markets (Gaba analysis).
///
/// | Time Remaining | Size |
/// |----------------|------|
/// | >3 min (>180s) | 12   |
/// | 2-3 min        | 11   |
/// | 1-2 min        | 9    |
/// | <1 min (<60s)  | 7    |
fn calc_size_5m(time_remaining_secs: i64) -> Decimal {
    if time_remaining_secs > 180 {
        dec!(12) // >3 min remaining
    } else if time_remaining_secs > 120 {
        dec!(11) // 2-3 min remaining
    } else if time_remaining_secs > 60 {
        dec!(9) // 1-2 min remaining
    } else {
        dec!(7) // <1 min remaining
    }
}

/// Sizing for 15-minute markets (estimated from Gaba).
///
/// Scaled up from 5m: roughly 2x the sizes.
///
/// | Time Remaining | Size |
/// |----------------|------|
/// | >9 min (>540s) | 24   |
/// | 6-9 min        | 20   |
/// | 3-6 min        | 16   |
/// | <3 min (<180s) | 12   |
fn calc_size_15m(time_remaining_secs: i64) -> Decimal {
    if time_remaining_secs > 540 {
        dec!(24) // >9 min remaining (60%)
    } else if time_remaining_secs > 360 {
        dec!(20) // 6-9 min remaining
    } else if time_remaining_secs > 180 {
        dec!(16) // 3-6 min remaining
    } else {
        dec!(12) // <3 min remaining
    }
}

/// Check if we should place orders on this side based on position limits.
///
/// Returns true if we can place, false if at limit.
///
/// # Arguments
/// * `side` - Side to check
/// * `position` - Current position
/// * `max_position` - Maximum allowed net position per side
pub fn can_place(side: Side, position: &Position, max_position: Decimal) -> bool {
    let net = position.net_position();
    let side_exposure = match side {
        Side::Yes => net,  // Positive = heavy YES
        Side::No => -net,  // Negative net = heavy NO, flip sign
    };
    side_exposure < max_position
}

/// Calculate size with position limit check.
///
/// Returns 0 if at position limit for this side.
pub fn calc_size_with_limit(
    side: Side,
    position: &Position,
    time_remaining_secs: i64,
    duration: MarketDuration,
    max_position: Decimal,
) -> Decimal {
    if !can_place(side, position, max_position) {
        return Decimal::ZERO;
    }
    calc_size(time_remaining_secs, duration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_5m_sizing() {
        // >3 min
        assert_eq!(calc_size_5m(250), dec!(12));
        assert_eq!(calc_size_5m(181), dec!(12));

        // 2-3 min
        assert_eq!(calc_size_5m(180), dec!(11));
        assert_eq!(calc_size_5m(121), dec!(11));

        // 1-2 min
        assert_eq!(calc_size_5m(120), dec!(9));
        assert_eq!(calc_size_5m(61), dec!(9));

        // <1 min
        assert_eq!(calc_size_5m(60), dec!(7));
        assert_eq!(calc_size_5m(30), dec!(7));
        assert_eq!(calc_size_5m(1), dec!(7));
    }

    #[test]
    fn test_15m_sizing() {
        // >9 min
        assert_eq!(calc_size_15m(600), dec!(24));
        assert_eq!(calc_size_15m(541), dec!(24));

        // 6-9 min
        assert_eq!(calc_size_15m(540), dec!(20));
        assert_eq!(calc_size_15m(361), dec!(20));

        // 3-6 min
        assert_eq!(calc_size_15m(360), dec!(16));
        assert_eq!(calc_size_15m(181), dec!(16));

        // <3 min
        assert_eq!(calc_size_15m(180), dec!(12));
        assert_eq!(calc_size_15m(60), dec!(12));
    }

    #[test]
    fn test_calc_size_dispatch() {
        assert_eq!(calc_size(200, MarketDuration::FiveMin), dec!(12));
        assert_eq!(calc_size(200, MarketDuration::FifteenMin), dec!(16));
    }

    #[test]
    fn test_can_place_within_limit() {
        let mut position = Position::default();
        position.apply_fill(Side::Yes, 500, dec!(30));
        position.apply_fill(Side::No, 500, dec!(20));
        // net = 10 (heavy YES)

        let max_pos = dec!(50);

        // YES side is heavy (net=10), but under limit
        assert!(can_place(Side::Yes, &position, max_pos));
        // NO side is light, definitely under limit
        assert!(can_place(Side::No, &position, max_pos));
    }

    #[test]
    fn test_can_place_at_limit() {
        let mut position = Position::default();
        position.apply_fill(Side::Yes, 500, dec!(50));
        // net = 50 (heavy YES)

        let max_pos = dec!(50);

        // YES at limit
        assert!(!can_place(Side::Yes, &position, max_pos));
        // NO is fine (net for NO perspective is -50, which is < 50)
        assert!(can_place(Side::No, &position, max_pos));
    }

    #[test]
    fn test_calc_size_with_limit() {
        let mut position = Position::default();
        position.apply_fill(Side::Yes, 500, dec!(60));
        // net = 60

        let max_pos = dec!(50);

        // YES over limit → 0
        assert_eq!(
            calc_size_with_limit(Side::Yes, &position, 200, MarketDuration::FiveMin, max_pos),
            Decimal::ZERO
        );

        // NO under limit → normal size
        assert_eq!(
            calc_size_with_limit(Side::No, &position, 200, MarketDuration::FiveMin, max_pos),
            dec!(12)
        );
    }
}
