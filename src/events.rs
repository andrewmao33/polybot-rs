// enum = "one of these things". Like a dropdown menu of possible event types.
// Each variant can carry different data.
pub enum Event {
    // Binance sends new BTC price
    BtcPrice { price: f64 },

    // Polymarket book update for ONE side (prices in ticks, 1 tick = 0.1 cent)
    BookUpdate {
        side: Side,
        bid: u16,
        ask: u16,
    },

    // One of our orders got filled
    OrderFill {
        order_id: String,
        side: Side,
        price: u16,
        size: f64,
    },

    // Timer tick (every second)
    Tick,

    // Ctrl+C or kill signal
    Shutdown,
}

// Another enum - just two options, no data attached
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Yes,
    No,
}

impl Side {
    /// Get the opposite side.
    pub fn opposite(&self) -> Side {
        match self {
            Side::Yes => Side::No,
            Side::No => Side::Yes,
        }
    }
}
