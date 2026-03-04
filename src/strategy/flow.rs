//! Flow estimator for order arrival rate (k).
//!
//! Measures trades per second on the Polymarket book.
//! Only counts actual trade executions (last_trade_price events),
//! NOT order placements or cancellations.

use std::collections::VecDeque;

/// Estimates order arrival rate (k) from trade events.
pub struct FlowEstimator {
    /// Timestamps of recent trades
    trade_timestamps: VecDeque<f64>,
    /// Rolling window in seconds
    window_secs: f64,
    /// Minimum k (never let flow premium collapse)
    k_floor: f64,
}

impl FlowEstimator {
    /// Create a new flow estimator.
    pub fn new(window_secs: f64, k_floor: f64) -> Self {
        Self {
            trade_timestamps: VecDeque::with_capacity(1000),
            window_secs,
            k_floor,
        }
    }

    /// Call ONLY on last_trade_price events from the Polymarket WebSocket.
    /// Do NOT call on book updates or price_change events.
    pub fn record_trade(&mut self, timestamp_secs: f64) {
        self.trade_timestamps.push_back(timestamp_secs);
        // Evict old trades outside the window
        while let Some(&front) = self.trade_timestamps.front() {
            if timestamp_secs - front > self.window_secs {
                self.trade_timestamps.pop_front();
            } else {
                break;
            }
        }
    }

    /// Trades per second over the rolling window.
    pub fn current_k(&self) -> f64 {
        let count = self.trade_timestamps.len() as f64;
        (count / self.window_secs).max(self.k_floor)
    }

    /// Get current trade count in window.
    pub fn trade_count(&self) -> usize {
        self.trade_timestamps.len()
    }

    /// Reset for new market window.
    pub fn reset(&mut self) {
        self.trade_timestamps.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let est = FlowEstimator::new(30.0, 0.1);
        assert_eq!(est.trade_count(), 0);
        assert!((est.current_k() - 0.1).abs() < 1e-9); // k_floor
    }

    #[test]
    fn test_record_single_trade() {
        let mut est = FlowEstimator::new(30.0, 0.1);
        est.record_trade(1.0);
        assert_eq!(est.trade_count(), 1);
    }

    #[test]
    fn test_window_eviction() {
        let mut est = FlowEstimator::new(30.0, 0.1);
        est.record_trade(1.0);
        est.record_trade(10.0);
        est.record_trade(20.0);
        assert_eq!(est.trade_count(), 3);

        // Record trade at t=35, should evict t=1.0
        est.record_trade(35.0);
        assert_eq!(est.trade_count(), 3); // t=10, t=20, t=35

        // Record trade at t=60, should evict t=10, t=20
        est.record_trade(60.0);
        assert_eq!(est.trade_count(), 2); // t=35, t=60
    }

    #[test]
    fn test_current_k() {
        let mut est = FlowEstimator::new(30.0, 0.1);
        // 30 trades in 30 seconds = 1 trade/sec
        for i in 0..30 {
            est.record_trade(i as f64);
        }
        let k = est.current_k();
        assert!((k - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_k_floor() {
        let est = FlowEstimator::new(30.0, 0.5);
        // No trades, should return k_floor
        assert!((est.current_k() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_reset() {
        let mut est = FlowEstimator::new(30.0, 0.1);
        est.record_trade(1.0);
        est.record_trade(2.0);
        est.reset();
        assert_eq!(est.trade_count(), 0);
    }
}
