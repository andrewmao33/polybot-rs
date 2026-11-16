//! BTC price-shock detection for risk management.
//!
//! Monitors BTC price movements and triggers order cancellation when a
//! sharp move in EITHER direction is detected. We always rest bids on both
//! YES and NO, so a sharp drop leaves our YES bid stale and a sharp rise
//! leaves our NO bid stale — both expose us to adverse selection.

use std::collections::VecDeque;

/// Configuration for shock detection.
#[derive(Debug, Clone)]
pub struct BtcGuardConfig {
    /// Maximum move (up or down) before triggering (e.g., 0.003 = 0.3%)
    pub max_move_pct: f64,
    /// Time window to check for moves (seconds)
    pub window_secs: f64,
    /// Cooldown after a shock before resuming (seconds)
    pub cooldown_secs: f64,
}

impl Default for BtcGuardConfig {
    fn default() -> Self {
        Self {
            max_move_pct: 0.003, // 0.3% move triggers
            window_secs: 2.0,    // Within 2 seconds
            cooldown_secs: 5.0,  // 5 second cooldown after shock
        }
    }
}

/// Tracks BTC prices and detects sharp moves.
pub struct BtcGuard {
    config: BtcGuardConfig,
    /// Ring buffer of (timestamp, price)
    prices: VecDeque<(f64, f64)>,
    /// Last shock timestamp (for cooldown)
    last_shock_ts: Option<f64>,
    /// Current price
    current_price: f64,
}

impl BtcGuard {
    pub fn new(config: BtcGuardConfig) -> Self {
        Self {
            config,
            prices: VecDeque::with_capacity(100),
            last_shock_ts: None,
            current_price: 0.0,
        }
    }

    /// Update with new BTC price. Call on each Event::BtcPrice.
    ///
    /// # Returns
    /// `true` if a shock (sharp drop OR rise) is detected and orders should
    /// be cancelled.
    pub fn update(&mut self, price: f64, now: f64) -> bool {
        self.current_price = price;

        // Check if we're in cooldown
        if let Some(last_shock) = self.last_shock_ts {
            if now - last_shock < self.config.cooldown_secs {
                // Still in cooldown - don't trigger again
                return false;
            }
        }

        // Add new price
        self.prices.push_back((now, price));

        // Remove old prices outside window
        let cutoff = now - self.config.window_secs;
        while let Some(&(ts, _)) = self.prices.front() {
            if ts < cutoff {
                self.prices.pop_front();
            } else {
                break;
            }
        }

        // Find the high and low of the window
        let (min_price, max_price) = self
            .prices
            .iter()
            .fold((f64::INFINITY, 0.0_f64), |(lo, hi), &(_, p)| (lo.min(p), hi.max(p)));

        if max_price > 0.0 && min_price > 0.0 {
            // Drop from recent high (stale YES bids) or rise from recent low (stale NO bids)
            let drop_pct = (max_price - price) / max_price;
            let rise_pct = (price - min_price) / min_price;
            if drop_pct >= self.config.max_move_pct || rise_pct >= self.config.max_move_pct {
                self.last_shock_ts = Some(now);
                return true;
            }
        }

        false
    }

    /// Check if we're currently in cooldown period.
    pub fn in_cooldown(&self, now: f64) -> bool {
        if let Some(last_shock) = self.last_shock_ts {
            now - last_shock < self.config.cooldown_secs
        } else {
            false
        }
    }

    /// Get current BTC price.
    pub fn current_price(&self) -> f64 {
        self.current_price
    }

    /// Reset state (call on market switch).
    pub fn reset(&mut self) {
        self.prices.clear();
        self.last_shock_ts = None;
        self.current_price = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> BtcGuardConfig {
        BtcGuardConfig {
            max_move_pct: 0.003,
            window_secs: 2.0,
            cooldown_secs: 5.0,
        }
    }

    #[test]
    fn test_no_shock_stable_price() {
        let mut guard = BtcGuard::new(cfg());

        // Stable price - no shock
        assert!(!guard.update(50000.0, 0.0));
        assert!(!guard.update(50000.0, 0.5));
        assert!(!guard.update(50000.0, 1.0));
        assert!(!guard.update(49990.0, 1.5)); // 0.02% drop - OK
        assert!(!guard.update(50010.0, 1.8)); // 0.04% rise from low - OK
    }

    #[test]
    fn test_drop_detected() {
        let mut guard = BtcGuard::new(cfg());

        // Price rises (0.2%, below threshold)
        assert!(!guard.update(50000.0, 0.0));
        assert!(!guard.update(50100.0, 0.5));

        // From 50100 to 49900 = -0.4% - shock!
        assert!(guard.update(49900.0, 1.0));
    }

    #[test]
    fn test_rise_detected() {
        let mut guard = BtcGuard::new(cfg());

        // Price dips (0.2%, below threshold)
        assert!(!guard.update(50000.0, 0.0));
        assert!(!guard.update(49900.0, 0.5));

        // From 49900 to 50100 = +0.4% - shock!
        assert!(guard.update(50100.0, 1.0));
        assert!(guard.in_cooldown(1.0));
    }

    #[test]
    fn test_cooldown_prevents_repeated_triggers() {
        let mut guard = BtcGuard::new(cfg());

        // Trigger shock
        guard.update(50000.0, 0.0);
        assert!(guard.update(49800.0, 1.0)); // -0.4%

        // Further moves during cooldown don't trigger again
        assert!(!guard.update(49700.0, 2.0));
        assert!(!guard.update(50200.0, 3.0));

        // Still in cooldown at 5s
        assert!(guard.in_cooldown(5.0));

        // Cooldown ends after 5s
        assert!(!guard.in_cooldown(6.0));
    }

    #[test]
    fn test_old_prices_expire() {
        let mut guard = BtcGuard::new(cfg());

        // High price at t=0, outside the 2s window by t=3
        assert!(!guard.update(50000.0, 0.0));
        assert!(!guard.update(49800.0, 3.0)); // No shock - old high expired

        // Low price at t=3, outside the window by t=6
        assert!(!guard.update(50000.0, 6.0)); // No shock - old low expired
    }

    #[test]
    fn test_reset() {
        let mut guard = BtcGuard::new(BtcGuardConfig::default());

        guard.update(50000.0, 0.0);
        guard.update(49800.0, 1.0);

        guard.reset();

        assert_eq!(guard.current_price(), 0.0);
        assert!(!guard.in_cooldown(0.0));
    }
}
