//! Live test for A-S pricing engine with market switching.
//!
//! Connects to Polymarket, computes quotes using variance and flow estimators.
//! Automatically switches to next market 5s before current one ends.
//! Only shows prices during valid quoting times (15s after open, mid 7-93%).
//!
//! Usage:
//!     cargo run --bin test_pricing

use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

use polybot_rs::api::gamma;
use polybot_rs::events::{Event, Side};
use polybot_rs::feeds::binance;
use polybot_rs::feeds::polymarket::PolymarketFeed;
use polybot_rs::strategy::{AvellanedaStoikov, FlowEstimator, Quotes, VarianceEstimator};

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

const WARMUP_SECS: f64 = 15.0;  // Wait 15s after market open before quoting
const SWITCH_EARLY_SECS: f64 = 15.0;  // Switch 15s before market ends (matches Gaba's last fills)

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== A-S Pricing Engine Live Test ===\n");

    // Get current market
    println!("Fetching current market...");
    let mut market = gamma::get_current_5m_market().await?;
    println!("Market: {}", market.slug);

    // Parse start epoch from slug
    let mut market_start: f64 = gamma::parse_start_epoch(&market.slug)
        .map(|e| e as f64)
        .unwrap_or(now_secs());
    let mut market_end: f64 = market_start + 300.0;

    println!("Start: {}, End: {} (in {:.0}s)\n", market_start, market_end, market_end - now_secs());

    // Create event channel
    let (tx, mut rx) = mpsc::channel::<Event>(1000);

    // Spawn feeds
    binance::spawn(tx.clone());
    let poly_feed = PolymarketFeed::new(market.yes_token.clone(), market.no_token.clone());
    let mut poly_handle = poly_feed.spawn(tx.clone());

    // Create estimators
    let mut var_est = VarianceEstimator::new(120, 0.01);
    let mut flow_est = FlowEstimator::new(30.0, 0.1);

    // Create A-S pricer
    let as_pricer = AvellanedaStoikov::new(0.07);

    // Track state
    let mut yes_bid: u16 = 0;
    let mut yes_ask: u16 = 0;
    let mut no_bid: u16 = 0;
    let mut no_ask: u16 = 0;
    let mut last_print = now_secs();
    let inventory: f64 = 0.0; // Assume no inventory for test

    // Track current market slug (for future use)
    let mut _current_slug = market.slug.clone();

    fn print_header() {
        println!("{}", "=".repeat(120));
        println!("{:>5} | {:>9} | {:>9} | {:>9} | {:>9} | {:>8} | {:>8} | {:>6}",
            "T", "Book YES", "Book NO", "A-S YES", "A-S NO", "Pair$", "Spread", "Var");
        println!("{:>5} | {:>9} | {:>9} | {:>9} | {:>9} | {:>8} | {:>8} | {:>6}",
            "", "bid/ask", "bid/ask", "bid", "bid", "", "", "");
        println!("{}", "=".repeat(120));
    }

    print_header();

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                let now = now_secs();
                let time_left = market_end - now;
                let market_age = now - market_start;

                // Check if we need to switch markets (5s before end)
                if time_left <= SWITCH_EARLY_SECS {
                    println!("\n>>> Switching to next market (T-{:.0}s)...", time_left);

                    // Fetch next market
                    match gamma::get_next_5m_market().await {
                        Ok(next_market) => {
                            market = next_market;
                            market_start = gamma::parse_start_epoch(&market.slug)
                                .map(|e| e as f64)
                                .unwrap_or(now_secs());
                            market_end = market_start + 300.0;

                            // Reset estimators
                            var_est.reset();
                            flow_est.reset();

                            // Reset book state
                            yes_bid = 0;
                            yes_ask = 0;
                            no_bid = 0;
                            no_ask = 0;

                            // Update current slug
                            _current_slug = market.slug.clone();

                            // Abort old feed and spawn new one
                            poly_handle.abort();
                            let new_feed = PolymarketFeed::new(
                                market.yes_token.clone(),
                                market.no_token.clone()
                            );
                            poly_handle = new_feed.spawn(tx.clone());

                            println!(">>> New market: {}", market.slug);
                            println!(">>> Waiting {:.0}s for warmup (until T+{:.0}s)...\n",
                                WARMUP_SECS - (now - market_start).max(0.0),
                                WARMUP_SECS);
                            print_header();
                        }
                        Err(e) => {
                            println!(">>> Failed to fetch next market: {}", e);
                            println!(">>> Retrying in 1s...");
                            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        }
                    }
                    continue;
                }

                // Process events
                match event {
                    Event::BtcPrice { .. } => {}
                    Event::BookUpdate { side, bid, ask } => {
                        match side {
                            Side::Yes => {
                                yes_bid = bid;
                                yes_ask = ask;
                                // Update variance estimator with canonical mid
                                if yes_bid > 0 && yes_ask > 0 {
                                    let mid = (yes_bid + yes_ask) as f64 / 2.0 / 1000.0;
                                    var_est.update_poly(mid, now);
                                }
                            }
                            Side::No => {
                                no_bid = bid;
                                no_ask = ask;
                            }
                        }
                    }
                    Event::Trade { .. } => {
                        flow_est.record_trade(now);
                    }
                    _ => {}
                }

                // Only print during valid times:
                // 1. After warmup (15s after market open)
                // 2. Every second
                // 3. Have valid book data
                if now - last_print >= 1.0 && yes_bid > 0 && yes_ask > 0 {
                    let mid = (yes_bid + yes_ask) as f64 / 2.0 / 1000.0;
                    let var = var_est.current_var();
                    let k = flow_est.current_k();

                    // Book prices in cents
                    let book_yes_bid_c = yes_bid as f64 / 10.0;
                    let book_yes_ask_c = yes_ask as f64 / 10.0;
                    let book_no_bid_c = no_bid as f64 / 10.0;
                    let book_no_ask_c = no_ask as f64 / 10.0;

                    // Check valid quoting conditions
                    let in_warmup = market_age < WARMUP_SECS;
                    let mid_valid = Quotes::should_quote(mid);

                    if in_warmup {
                        // Still warming up - show but indicate not quoting
                        println!("{:>5.0} | {:>4.1}/{:<4.1} | {:>4.1}/{:<4.1} | {:>9} | {:>8} | {:>8} | {:>7} | {:>6.4}  [WARMUP: {:.0}s left]",
                            time_left,
                            book_yes_bid_c, book_yes_ask_c,
                            book_no_bid_c, book_no_ask_c,
                            "---", "---", "---", "---",
                            var,
                            WARMUP_SECS - market_age
                        );
                    } else if !mid_valid {
                        // Mid outside 7-93% range
                        println!("{:>5.0} | {:>4.1}/{:<4.1} | {:>4.1}/{:<4.1} | {:>9} | {:>8} | {:>8} | {:>7} | {:>6.4}  [NO QUOTE: mid={:.0}%]",
                            time_left,
                            book_yes_bid_c, book_yes_ask_c,
                            book_no_bid_c, book_no_ask_c,
                            "---", "---", "---", "---",
                            var,
                            mid * 100.0
                        );
                    } else {
                        // Valid quoting time - compute and show quotes
                        let quotes = as_pricer.compute_quotes(mid, inventory, var, k, time_left);
                        let pair_cost = quotes.yes_bid + quotes.no_bid;
                        let spread_pct = (1.0 - pair_cost) * 100.0;

                        let as_yes_cents = AvellanedaStoikov::to_cents(quotes.yes_bid);
                        let as_no_cents = AvellanedaStoikov::to_cents(quotes.no_bid);

                        println!("{:>5.0} | {:>4.1}/{:<4.1} | {:>4.1}/{:<4.1} | {:>9}c | {:>8}c | {:>8.4} | {:>6.2}% | {:>6.4}",
                            time_left,
                            book_yes_bid_c, book_yes_ask_c,
                            book_no_bid_c, book_no_ask_c,
                            as_yes_cents,
                            as_no_cents,
                            pair_cost,
                            spread_pct,
                            var
                        );
                    }

                    last_print = now;
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
