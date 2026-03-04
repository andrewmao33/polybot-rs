//! Variance estimator for Avellaneda-Stoikov pricing.
//!
//! Computes `var` — the variance of logit probability increments per second.
//! This is the most important input to the pricing model.

use std::collections::VecDeque;

/// Estimates variance of probability movements in logit space.
pub struct VarianceEstimator {
    /// Rolling logit increments (dx = x_t - x_{t-1})
    recent_dx: VecDeque<f64>,
    /// Timestamps for each increment (for time-scaling)
    recent_timestamps: VecDeque<f64>,
    /// Number of ticks to keep in rolling window
    window_size: usize,
    /// Last logit value
    last_logit: Option<f64>,
    /// Last timestamp
    last_timestamp: Option<f64>,
    /// Minimum variance (never let spread collapse)
    var_floor: f64,
}

impl VarianceEstimator {
    /// Create a new variance estimator with given parameters.
    pub fn new(window_size: usize, var_floor: f64) -> Self {
        Self {
            recent_dx: VecDeque::with_capacity(window_size),
            recent_timestamps: VecDeque::with_capacity(window_size),
            window_size,
            last_logit: None,
            last_timestamp: None,
            var_floor,
        }
    }

    /// Call this every time the Polymarket canonical mid updates.
    pub fn update_poly(&mut self, p_t: f64, timestamp_secs: f64) {
        // Clamp p_t away from 0 and 1 to avoid logit infinity
        let p_clamped = p_t.clamp(0.01, 0.99);
        let x_t = (p_clamped / (1.0 - p_clamped)).ln();

        if let (Some(x_prev), Some(t_prev)) = (self.last_logit, self.last_timestamp) {
            let dx = x_t - x_prev;
            let dt = timestamp_secs - t_prev;
            if dt > 0.0 {
                self.recent_dx.push_back(dx);
                self.recent_timestamps.push_back(dt);
                if self.recent_dx.len() > self.window_size {
                    self.recent_dx.pop_front();
                    self.recent_timestamps.pop_front();
                }
            }
        }

        self.last_logit = Some(x_t);
        self.last_timestamp = Some(timestamp_secs);
    }

    /// Variance of logit increments per second. Floored at var_floor.
    pub fn current_var(&self) -> f64 {
        if self.recent_dx.len() < 5 {
            return self.var_floor;
        }

        let n = self.recent_dx.len() as f64;
        let mean = self.recent_dx.iter().sum::<f64>() / n;
        let var_per_tick = self.recent_dx
            .iter()
            .map(|d| (d - mean).powi(2))
            .sum::<f64>()
            / (n - 1.0);

        let avg_dt = self.recent_timestamps.iter().sum::<f64>() / n;
        if avg_dt > 0.0 {
            (var_per_tick / avg_dt).max(self.var_floor)
        } else {
            self.var_floor
        }
    }

    /// Reset for new market window. Optionally carry forward last var.
    pub fn reset(&mut self) {
        self.recent_dx.clear();
        self.recent_timestamps.clear();
        self.last_logit = None;
        self.last_timestamp = None;
    }

    /// Get current sample count.
    pub fn sample_count(&self) -> usize {
        self.recent_dx.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let est = VarianceEstimator::new(60, 0.001);
        assert_eq!(est.window_size, 60);
        assert_eq!(est.sample_count(), 0);
    }

    #[test]
    fn test_update_single() {
        let mut est = VarianceEstimator::new(60, 0.001);
        est.update_poly(0.5, 1.0);
        // First update, no increment yet
        assert_eq!(est.sample_count(), 0);
        assert!(est.last_logit.is_some());
    }

    #[test]
    fn test_update_multiple() {
        let mut est = VarianceEstimator::new(60, 0.001);
        est.update_poly(0.5, 1.0);
        est.update_poly(0.51, 2.0);
        est.update_poly(0.49, 3.0);
        assert_eq!(est.sample_count(), 2);
    }

    #[test]
    fn test_var_floor_when_insufficient_samples() {
        let mut est = VarianceEstimator::new(60, 0.001);
        est.update_poly(0.5, 1.0);
        est.update_poly(0.51, 2.0);
        // Only 1 sample, need at least 5
        let var = est.current_var();
        assert!((var - 0.001).abs() < 1e-9); // var_floor
    }

    #[test]
    fn test_reset() {
        let mut est = VarianceEstimator::new(60, 0.001);
        est.update_poly(0.5, 1.0);
        est.update_poly(0.51, 2.0);
        est.reset();
        assert_eq!(est.sample_count(), 0);
        assert!(est.last_logit.is_none());
    }

    #[test]
    fn test_window_rolls() {
        let mut est = VarianceEstimator::new(3, 0.001);
        est.update_poly(0.50, 1.0);
        est.update_poly(0.51, 2.0);
        est.update_poly(0.52, 3.0);
        est.update_poly(0.53, 4.0);
        est.update_poly(0.54, 5.0);
        // Window size is 3, should only have 3 samples
        assert_eq!(est.sample_count(), 3);
    }
}
