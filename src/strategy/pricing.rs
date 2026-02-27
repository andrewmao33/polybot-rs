use crate::events::Side;
use crate::state::Book;

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
}
