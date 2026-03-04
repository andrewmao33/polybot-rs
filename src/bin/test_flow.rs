//! Live test for flow estimator (k).
//!
//! Connects to Polymarket, counts trades per second.
//!
//! Usage:
//!     cargo run --bin test_flow

use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

use polybot_rs::api::gamma;
use polybot_rs::events::{Event, Side};
use polybot_rs::feeds::polymarket::PolymarketFeed;
use polybot_rs::strategy::FlowEstimator;

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== Flow Estimator Live Test ===\n");

    // Get current market
    println!("Fetching current market...");
    let market = gamma::get_current_5m_market().await?;
    println!("Market: {}", market.slug);
    println!("YES: {}...", &market.yes_token[..20]);
    println!("NO: {}...\n", &market.no_token[..20]);

    // Create event channel
    let (tx, mut rx) = mpsc::channel::<Event>(1000);

    // Spawn Polymarket feed
    let poly_feed = PolymarketFeed::new(market.yes_token.clone(), market.no_token.clone());
    poly_feed.spawn(tx.clone());

    // Create flow estimator (30 second window, k_floor = 0.1)
    let mut flow_est = FlowEstimator::new(30.0, 0.1);

    // Track state
    let mut yes_trades: u64 = 0;
    let mut no_trades: u64 = 0;
    let mut last_print = now_secs();

    println!("{}", "=".repeat(60));
    println!("{:>8} | {:>8} | {:>10} | {:>10} | {:>10}",
        "YES", "NO", "Total", "In Window", "k");
    println!("{}", "=".repeat(60));

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                match event {
                    Event::Trade { side, price } => {
                        let now = now_secs();
                        flow_est.record_trade(now);

                        match side {
                            Side::Yes => yes_trades += 1,
                            Side::No => no_trades += 1,
                        }

                        // Print every second
                        if now - last_print >= 1.0 {
                            let k = flow_est.current_k();
                            let in_window = flow_est.trade_count();

                            println!("{:>8} | {:>8} | {:>10} | {:>10} | {:>10.3}",
                                yes_trades,
                                no_trades,
                                yes_trades + no_trades,
                                in_window,
                                k
                            );
                            last_print = now;
                        }

                        // Also print each trade
                        let side_str = match side {
                            Side::Yes => "YES",
                            Side::No => "NO",
                        };
                        println!("  [TRADE] {} @ {:.3}", side_str, price as f64 / 1000.0);
                    }
                    Event::BookUpdate { .. } => {
                        // Ignore book updates for this test
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
