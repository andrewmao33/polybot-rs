use crate::events::Side;

/// Order book state - best bid/ask for YES and NO.
/// Prices are in ticks (0-1000, where 1000 = $1.00).
#[derive(Debug, Clone, Default)]
pub struct Book {
    pub yes_bid: Option<u16>,
    pub yes_ask: Option<u16>,
    pub no_bid: Option<u16>,
    pub no_ask: Option<u16>,
    /// Timestamp of last update (milliseconds)
    pub last_update_ms: i64,
}

impl Book {
    /// Check if we have valid data for both sides.
    /// Don't trade until this returns true.
    pub fn is_synced(&self) -> bool {
        self.yes_ask.is_some() && self.no_ask.is_some()
    }

    /// Update one side of the book.
    pub fn update(&mut self, side: Side, bid: u16, ask: u16, timestamp_ms: i64) {
        match side {
            Side::Yes => {
                self.yes_bid = Some(bid);
                self.yes_ask = Some(ask);
            }
            Side::No => {
                self.no_bid = Some(bid);
                self.no_ask = Some(ask);
            }
        }
        self.last_update_ms = timestamp_ms;
    }

    /// Get best ask for a side.
    pub fn best_ask(&self, side: Side) -> Option<u16> {
        match side {
            Side::Yes => self.yes_ask,
            Side::No => self.no_ask,
        }
    }

    /// Get best bid for a side.
    pub fn best_bid(&self, side: Side) -> Option<u16> {
        match side {
            Side::Yes => self.yes_bid,
            Side::No => self.no_bid,
        }
    }

    /// Get the opposite side's best ask.
    /// Used for pricing: max_bid = 1000 - opposite_ask - margin
    pub fn opposite_ask(&self, side: Side) -> Option<u16> {
        match side {
            Side::Yes => self.no_ask,
            Side::No => self.yes_ask,
        }
    }

    /// Reset book state (e.g., on market switch).
    pub fn reset(&mut self) {
        self.yes_bid = None;
        self.yes_ask = None;
        self.no_bid = None;
        self.no_ask = None;
        self.last_update_ms = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_not_synced() {
        let book = Book::default();
        assert!(!book.is_synced());
    }

    #[test]
    fn test_partial_sync() {
        let mut book = Book::default();

        book.update(Side::Yes, 480, 490, 1000);
        assert!(!book.is_synced()); // Missing NO

        book.update(Side::No, 500, 510, 1001);
        assert!(book.is_synced()); // Now have both
    }

    #[test]
    fn test_update() {
        let mut book = Book::default();

        book.update(Side::Yes, 480, 490, 1000);
        assert_eq!(book.yes_bid, Some(480));
        assert_eq!(book.yes_ask, Some(490));
        assert_eq!(book.last_update_ms, 1000);

        book.update(Side::No, 500, 510, 1001);
        assert_eq!(book.no_bid, Some(500));
        assert_eq!(book.no_ask, Some(510));
        assert_eq!(book.last_update_ms, 1001);
    }

    #[test]
    fn test_opposite_ask() {
        let mut book = Book::default();
        book.update(Side::Yes, 480, 490, 1000);
        book.update(Side::No, 500, 510, 1001);

        // For pricing YES: look at NO's ask
        assert_eq!(book.opposite_ask(Side::Yes), Some(510));
        // For pricing NO: look at YES's ask
        assert_eq!(book.opposite_ask(Side::No), Some(490));
    }

    #[test]
    fn test_reset() {
        let mut book = Book::default();
        book.update(Side::Yes, 480, 490, 1000);
        book.update(Side::No, 500, 510, 1001);

        book.reset();
        assert!(!book.is_synced());
        assert_eq!(book.yes_ask, None);
        assert_eq!(book.no_ask, None);
    }
}
