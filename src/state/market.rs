/// Market identity - set once when switching to a new market.
#[derive(Debug, Clone)]
pub struct Market {
    /// Condition ID (e.g., "0x1234...")
    pub market_id: String,
    /// YES token ID for API calls
    pub token_id_yes: String,
    /// NO token ID for API calls
    pub token_id_no: String,
    /// Market slug (e.g., "btc-updown-15m-1739884800")
    pub slug: String,
    /// Market expiration timestamp in milliseconds
    pub end_timestamp_ms: i64,
}

impl Market {
    pub fn new(
        market_id: String,
        token_id_yes: String,
        token_id_no: String,
        slug: String,
        end_timestamp_ms: i64,
    ) -> Self {
        Self {
            market_id,
            token_id_yes,
            token_id_no,
            slug,
            end_timestamp_ms,
        }
    }

    /// Time remaining until market expiration in seconds.
    /// Returns 0 if market has ended.
    pub fn time_remaining_secs(&self, now_ms: i64) -> i64 {
        ((self.end_timestamp_ms - now_ms) / 1000).max(0)
    }
}
