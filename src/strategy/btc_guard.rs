//! BTC crash detection for risk management.
//!
//! Monitors BTC price movements and triggers order cancellation
//! when a significant drop is detected. This protects against
//! being filled on stale quotes when BTC moves sharply.

use std::collections::VecDeque;

/// Configuration for crash detection.
#[derive(Debug, Clone)]
pub struct BtcGuardConfig {
    /// Maximum price drop before triggering (e.g., 0.003 = 0.3%)
    pub max_drop_pct: f64,
    /// Time window to check for drops (seconds)
    pub window_secs: f64,
    /// Cooldown after crash before resuming (seconds)
    pub cooldown_secs: f64,
}

impl Default for BtcGuardConfig {
    fn default() -> Self {
        Self {
            max_drop_pct: 0.003,   // 0.3% drop triggers
            window_secs: 2.0,      // Within 2 seconds
            cooldown_secs: 5.0,    // 5 second cooldown after crash
        }
    }
}

/// Tracks BTC prices and detects crashes.
pub struct BtcGuard {
    config: BtcGuardConfig,
    /// Ring buffer of (timestamp, price)
    prices: VecDeque<(f64, f64)>,
    /// Last crash timestamp (for cooldown)
    last_crash_ts: Option<f64>,
    /// Current price
    current_price: f64,
}

impl BtcGuard {
    pub fn new(config: BtcGuardConfig) -> Self {
        Self {
            config,
            prices: VecDeque::with_capacity(100),
            last_crash_ts: None,
            current_price: 0.0,
        }
    }

    /// Update with new BTC price. Call on each Event::BtcPrice.
    ///
    /// # Returns
    /// `true` if a crash is detected and orders should be cancelled.
    pub fn update(&mut self, price: f64, now: f64) -> bool {
        self.current_price = price;

        // Check if we're in cooldown
        if let Some(last_crash) = self.last_crash_ts {
            if now - last_crash < self.config.cooldown_secs {
                // Still in cooldown - don't trigger again but also don't clear
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

        // Check for crash: find max price in window and compare to current
        let max_price = self
            .prices
            .iter()
            .map(|(_, p)| *p)
            .fold(0.0_f64, |a, b| a.max(b));

        if max_price > 0.0 {
            let drop_pct = (max_price - price) / max_price;
            if drop_pct >= self.config.max_drop_pct {
                self.last_crash_ts = Some(now);
                return true;
            }
        }

        false
    }

    /// Check if we're currently in cooldown period.
    pub fn in_cooldown(&self, now: f64) -> bool {
        if let Some(last_crash) = self.last_crash_ts {
            now - last_crash < self.config.cooldown_secs
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
        self.last_crash_ts = None;
        self.current_price = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_crash_stable_price() {
        let config = BtcGuardConfig {
            max_drop_pct: 0.003,
            window_secs: 2.0,
            cooldown_secs: 5.0,
        };
        let mut guard = BtcGuard::new(config);

        // Stable price - no crash
        assert!(!guard.update(50000.0, 0.0));
        assert!(!guard.update(50000.0, 0.5));
        assert!(!guard.update(50000.0, 1.0));
        assert!(!guard.update(49990.0, 1.5)); // 0.02% drop - OK
    }

    #[test]
    fn test_crash_detected() {
        let config = BtcGuardConfig {
            max_drop_pct: 0.003, // 0.3%
            window_secs: 2.0,
            cooldown_secs: 5.0,
        };
        let mut guard = BtcGuard::new(config);

        // Price rises
        assert!(!guard.update(50000.0, 0.0));
        assert!(!guard.update(50100.0, 0.5));

        // Price drops 0.4% - crash!
        // From 50100 to 49900 = -0.4%
        assert!(guard.update(49900.0, 1.0));
    }

    #[test]
    fn test_cooldown_prevents_repeated_triggers() {
        let config = BtcGuardConfig {
            max_drop_pct: 0.003,
            window_secs: 2.0,
            cooldown_secs: 5.0,
        };
        let mut guard = BtcGuard::new(config);

        // Trigger crash
        guard.update(50000.0, 0.0);
        assert!(guard.update(49800.0, 1.0)); // -0.4% crash

        // Further drops during cooldown don't trigger again
        assert!(!guard.update(49700.0, 2.0));
        assert!(!guard.update(49600.0, 3.0));

        // Still in cooldown at 5s
        assert!(guard.in_cooldown(5.0));

        // Cooldown ends after 5s
        assert!(!guard.in_cooldown(6.0));
    }

    #[test]
    fn test_old_prices_expire() {
        let config = BtcGuardConfig {
            max_drop_pct: 0.003,
            window_secs: 2.0,
            cooldown_secs: 5.0,
        };
        let mut guard = BtcGuard::new(config);

        // High price at t=0
        assert!(!guard.update(50000.0, 0.0));

        // Price drops but outside window at t=3
        // Window is 2s, so t=0 price should be gone
        assert!(!guard.update(49800.0, 3.0)); // No crash - old high expired
    }

    #[test]
    fn test_reset() {
        let config = BtcGuardConfig::default();
        let mut guard = BtcGuard::new(config);

        guard.update(50000.0, 0.0);
        guard.update(49800.0, 1.0);

        guard.reset();

        assert_eq!(guard.current_price(), 0.0);
        assert!(!guard.in_cooldown(0.0));
    }
}
