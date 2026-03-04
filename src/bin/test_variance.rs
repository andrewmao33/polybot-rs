//! Live test for variance estimator.
//!
//! Connects to Polymarket and Binance, prints variance continuously.
//!
//! Usage:
//!     cargo run --bin test_variance

use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

use polybot_rs::api::gamma;
use polybot_rs::events::{Event, Side};
use polybot_rs::feeds::binance;
use polybot_rs::feeds::polymarket::PolymarketFeed;
use polybot_rs::strategy::VarianceEstimator;

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

/// Simple Binance variance tracker
struct BinanceVar {
    recent_dx: VecDeque<f64>,
    recent_dt: VecDeque<f64>,
    last_price: Option<f64>,
    last_time: Option<f64>,
    window_size: usize,
}

impl BinanceVar {
    fn new(window_size: usize) -> Self {
        Self {
            recent_dx: VecDeque::with_capacity(window_size),
            recent_dt: VecDeque::with_capacity(window_size),
            last_price: None,
            last_time: None,
            window_size,
        }
    }

    fn update(&mut self, price: f64, timestamp: f64) {
        if let (Some(prev_price), Some(prev_time)) = (self.last_price, self.last_time) {
            let dt = timestamp - prev_time;
            if dt > 0.0 && prev_price > 0.0 {
                let dx = (price / prev_price).ln(); // log return
                self.recent_dx.push_back(dx);
                self.recent_dt.push_back(dt);
                if self.recent_dx.len() > self.window_size {
                    self.recent_dx.pop_front();
                    self.recent_dt.pop_front();
                }
            }
        }
        self.last_price = Some(price);
        self.last_time = Some(timestamp);
    }

    fn variance(&self) -> f64 {
        if self.recent_dx.len() < 5 {
            return 0.0;
        }
        let n = self.recent_dx.len() as f64;
        let mean = self.recent_dx.iter().sum::<f64>() / n;
        let var_per_tick = self.recent_dx
            .iter()
            .map(|d| (d - mean).powi(2))
            .sum::<f64>() / (n - 1.0);
        let avg_dt = self.recent_dt.iter().sum::<f64>() / n;
        if avg_dt > 0.0 { var_per_tick / avg_dt } else { 0.0 }
    }

    fn sample_count(&self) -> usize {
        self.recent_dx.len()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Variance Estimator Live Test ===\n");

    // Get current market
    println!("Fetching current market...");
    let market = gamma::get_current_5m_market().await?;
    println!("Market: {}", market.slug);
    println!("YES: {}...", &market.yes_token[..20]);
    println!("NO: {}...", &market.no_token[..20]);

    // Parse strike from slug (e.g., "btc-updown-5m-1772234100")
    // For now, we'll get strike from the current BTC price (approximate)
    let strike: f64 = 97000.0; // Will be updated from Binance
    println!("Strike (approx): ${:.0}", strike);

    // Create event channel
    let (tx, mut rx) = mpsc::channel::<Event>(1000);

    // Spawn Binance feed
    binance::spawn(tx.clone());

    // Spawn Polymarket feed
    let poly_feed = PolymarketFeed::new(market.yes_token.clone(), market.no_token.clone());
    poly_feed.spawn(tx.clone());

    // Create variance estimators
    let mut var_est = VarianceEstimator::new(60, 0.001);
    let mut binance_var_est = BinanceVar::new(60);

    // Track state
    let mut btc_price: f64 = strike;
    let mut yes_bid: u16 = 0;
    let mut yes_ask: u16 = 0;
    let mut tick_count: u64 = 0;

    println!("\n{}", "=".repeat(70));
    println!("{:>6} | {:>10} | {:>8} | {:>8} | {:>14}",
        "Ticks", "BTC", "Mid", "Samples", "Variance");
    println!("{}", "=".repeat(70));

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                match event {
                    Event::BtcPrice { price } => {
                        btc_price = price;
                        binance_var_est.update(price, now_secs());
                    }
                    Event::BookUpdate { side, bid, ask } => {
                        if side == Side::Yes {
                            yes_bid = bid;
                            yes_ask = ask;

                            // Calculate mid probability
                            if yes_bid > 0 && yes_ask > 0 {
                                let mid = (yes_bid + yes_ask) as f64 / 2.0 / 1000.0;

                                // Update variance estimator
                                var_est.update_poly(mid, now_secs());
                                tick_count += 1;

                                // Get variance
                                let var = var_est.current_var();

                                // Print every 10 ticks
                                if tick_count % 10 == 0 {
                                    println!("{:>6} | {:>10.1} | {:>8.3} | {:>8} | {:>14.6e}",
                                        tick_count,
                                        btc_price,
                                        mid,
                                        var_est.sample_count(),
                                        var
                                    );
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!("\nShutting down...");
                break;
            }
        }
    }

    Ok(())
}
