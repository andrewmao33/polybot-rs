//! Polymarket BTC 5-minute market maker.
//!
//! Usage:
//!     cargo run                          # Live trading (indefinite)
//!     cargo run -- --log-only            # Dry run (no orders)
//!     cargo run -- --markets 3           # Trade 3 markets then quit
//!     cargo run -- --log-only --markets 1
//!
//! Required env vars:
//!     POLY_PRIVATE_KEY=0x...
//!     POLY_PROXY_WALLET=0x...

mod api;
mod config;
mod events;
mod executor;
mod feeds;
mod logging;
mod state;
mod strategy;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy_primitives::Address;
use anyhow::Result;
use polyfill_rs::orders::SigType;
use polyfill_rs::ClobClient;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::str::FromStr;
use tokio::sync::mpsc;
use tokio::time::interval;

use api::gamma;
use events::{Event, Side};
use executor::{Executor, ExecutorConfig};
use feeds::binance;
use feeds::polymarket::PolymarketFeed;
use feeds::user_ws::{UserFeed, UserFeedConfig};
use logging::{Logger, SessionStats, WindowStats};
use state::{Book, OrderTracker, Position};
use strategy::{
    Action, AvellanedaStoikov, BtcGuard, BtcGuardConfig, FlowEstimator, Quotes, VarianceEstimator,
};

// =============================================================================
// TUNABLE CONSTANTS
// =============================================================================

/// Timing
const TICK_MS: u64 = 50;           // Order management interval
const WARMUP_SECS: f64 = 15.0;     // Wait after market open
const HALT_SECS: f64 = 15.0;       // Stop before market ends

/// A-S Pricer
const AS_GAMMA: f64 = 0.15;        // Risk aversion (higher = wider spreads)
const NO_CROSS_MARGIN: u16 = 10;   // Don't bid within 1c of market ask (stay maker)

/// Variance Estimator
const VAR_WINDOW: usize = 120;     // Rolling window size (samples)
const VAR_FLOOR: f64 = 0.001;      // Minimum variance

/// Flow Estimator
const FLOW_WINDOW_SECS: f64 = 30.0; // Trade counting window
const FLOW_K_FLOOR: f64 = 0.1;      // Minimum k (trades/sec)

/// BTC Guard
const BTC_MAX_DROP_PCT: f64 = 0.003;  // 0.3% triggers cancel
const BTC_WINDOW_SECS: f64 = 2.0;     // Drop detection window
const BTC_COOLDOWN_SECS: f64 = 5.0;   // Pause after crash

/// Order size
const ORDER_SIZE: i64 = 5;         // Shares per order

/// Staleness detection
const STALE_MS: i64 = 5000;        // Halt if no book update for 5s

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

/// Round ticks to cent boundary (multiple of 10).
fn round_to_cents(ticks: u16) -> u16 {
    (ticks / 10) * 10
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    // Parse args
    let args: Vec<String> = std::env::args().collect();
    let log_only = args.iter().any(|a| a == "--log-only" || a == "--dry-run");

    // Parse --markets N
    let max_markets: Option<u32> = args.iter()
        .position(|a| a == "--markets")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok());

    if log_only {
        println!("=== DRY RUN MODE (no orders will be placed) ===");
    } else {
        println!("=== LIVE TRADING MODE ===");
    }
    if let Some(n) = max_markets {
        println!("Will trade {} market(s) then quit", n);
    }
    println!();

    // Load credentials from env
    let private_key = std::env::var("POLY_PRIVATE_KEY").expect("Set POLY_PRIVATE_KEY");
    let proxy_wallet = std::env::var("POLY_PROXY_WALLET").expect("Set POLY_PROXY_WALLET");
    let funder = Address::from_str(&proxy_wallet)?;

    // Create CLOB client and derive API credentials
    println!("Creating CLOB client...");
    let l1_client = ClobClient::with_l1_headers("https://clob.polymarket.com", &private_key, 137);
    let api_creds = l1_client.create_or_derive_api_key(None).await?;
    println!("API Key: {}...", &api_creds.api_key[..20.min(api_creds.api_key.len())]);

    // Save credentials for UserFeed before moving to ClobClient
    let user_api_key = api_creds.api_key.clone();
    let user_api_secret = api_creds.secret.clone();
    let user_api_passphrase = api_creds.passphrase.clone();

    let client = ClobClient::with_l2_headers(
        "https://clob.polymarket.com",
        &private_key,
        137,
        api_creds,
        Some(SigType::PolyProxy),
        Some(funder),
    );

    // Get current market and wait for next one to start fresh
    println!("Fetching current market...");
    let mut market = gamma::get_current_5m_market().await?;
    let mut market_start = gamma::parse_start_epoch(&market.slug)
        .map(|e| e as f64)
        .unwrap_or(now_secs());
    let mut market_end = market_start + 300.0;

    // Wait for next market if we're mid-market
    let time_left = market_end - now_secs();
    if time_left < 300.0 - WARMUP_SECS {
        println!("Current market {} has {:.0}s left, waiting for next...", market.slug, time_left);
        // Wait until this market ends + a bit
        tokio::time::sleep(Duration::from_secs_f64(time_left + 1.0)).await;

        // Fetch the new market
        market = gamma::get_current_5m_market().await?;
        market_start = gamma::parse_start_epoch(&market.slug)
            .map(|e| e as f64)
            .unwrap_or(now_secs());
        market_end = market_start + 300.0;
    }

    println!("Market: {}", market.slug);
    println!(
        "Start: {:.0}, End: {:.0} (in {:.0}s)\n",
        market_start,
        market_end,
        market_end - now_secs()
    );

    // Track markets completed
    let mut markets_completed: u32 = 0;

    // Create executor
    let executor_config = ExecutorConfig {
        log_only,
        yes_token: market.yes_token.clone(),
        no_token: market.no_token.clone(),
    };
    let mut executor = Executor::new(client, executor_config);

    // Create logger and stats
    let mut logger = Logger::new()?;
    let mut session_stats = SessionStats::new();
    let mut window_stats = WindowStats::new();

    // Create event channel
    let (tx, mut rx) = mpsc::channel::<Event>(1000);

    // Spawn feeds
    binance::spawn(tx.clone());
    let poly_feed = PolymarketFeed::new(market.yes_token.clone(), market.no_token.clone());
    let mut poly_handle = poly_feed.spawn(tx.clone());

    // Spawn user WebSocket for fill notifications
    let user_feed_config = UserFeedConfig {
        api_key: user_api_key,
        api_secret: user_api_secret,
        api_passphrase: user_api_passphrase,
        maker_address: proxy_wallet,
        yes_token: market.yes_token.clone(),
        no_token: market.no_token.clone(),
    };
    let user_feed = UserFeed::new(user_feed_config);
    user_feed.spawn(tx.clone());

    // Create state
    let mut book = Book::default();
    let mut position = Position::default();
    let mut orders = OrderTracker::new();

    // Create estimators (using constants from top of file)
    let mut var_est = VarianceEstimator::new(VAR_WINDOW, VAR_FLOOR);
    let mut flow_est = FlowEstimator::new(FLOW_WINDOW_SECS, FLOW_K_FLOOR);
    let mut btc_guard = BtcGuard::new(BtcGuardConfig {
        max_drop_pct: BTC_MAX_DROP_PCT,
        window_secs: BTC_WINDOW_SECS,
        cooldown_secs: BTC_COOLDOWN_SECS,
    });

    // Create A-S pricer
    let as_pricer = AvellanedaStoikov::new(AS_GAMMA);

    // 50ms tick interval
    let mut tick_interval = interval(Duration::from_millis(TICK_MS));
    let mut last_btc_price: f64 = 0.0;

    println!("Starting event loop... (Ctrl+C to quit)\n");
    logger.window_start(&market.slug);

    loop {
        tokio::select! {
            // 50ms strategy tick
            _ = tick_interval.tick() => {
                let now = now_secs();
                let time_left = market_end - now;
                let market_age = now - market_start;

                // Check if we need to switch markets
                if time_left <= HALT_SECS {
                    // Cancel all orders before switching
                    if orders.total_count() > 0 {
                        let cancelled = orders.total_count();
                        logger.halt(time_left, &market.slug, 0.5, var_est.current_var(), "MARKET_END", cancelled);
                        let actions = vec![Action::CancelAll];
                        let _ = executor.execute(actions, &mut orders).await;
                        session_stats.orders_cancelled += cancelled as u32;
                    }

                    // Wait for market to actually end
                    if time_left > 0.0 {
                        continue;
                    }

                    // End current window
                    window_stats.finalize();
                    let yes_shares = position.qty_yes.to_string().parse::<f64>().unwrap_or(0.0);
                    let yes_avg = position.avg_price_yes().map(|p| p.to_string().parse::<f64>().unwrap_or(0.0) / 10.0).unwrap_or(0.0);
                    let no_shares = position.qty_no.to_string().parse::<f64>().unwrap_or(0.0);
                    let no_avg = position.avg_price_no().map(|p| p.to_string().parse::<f64>().unwrap_or(0.0) / 10.0).unwrap_or(0.0);
                    let min_pnl = position.min_pnl_usd().to_string().parse::<f64>().unwrap_or(0.0);
                    logger.window_end(
                        &market.slug,
                        &window_stats,
                        yes_shares,
                        yes_avg,
                        no_shares,
                        no_avg,
                        min_pnl,
                        AS_GAMMA,
                        HALT_SECS,
                    );
                    session_stats.merge_window(&window_stats);
                    markets_completed += 1;

                    // Check if we should quit
                    if let Some(max) = max_markets {
                        if markets_completed >= max {
                            println!("\n>>> Completed {} market(s), shutting down...", markets_completed);
                            break;
                        }
                    }

                    // Fetch current market (which is now the new one since we're at T-0)
                    match gamma::get_current_5m_market().await {
                        Ok(new_market) => {
                            market = new_market;
                            market_start = gamma::parse_start_epoch(&market.slug)
                                .map(|e| e as f64)
                                .unwrap_or(now_secs());
                            market_end = market_start + 300.0;

                            // Reset state
                            var_est.reset();
                            flow_est.reset();
                            btc_guard.reset();
                            position.reset();
                            orders.clear_all();
                            book = Book::default();
                            window_stats = WindowStats::new();

                            // Update executor tokens
                            executor.set_market(market.yes_token.clone(), market.no_token.clone());

                            // Restart polymarket feed
                            poly_handle.abort();
                            let new_feed = PolymarketFeed::new(
                                market.yes_token.clone(),
                                market.no_token.clone(),
                            );
                            poly_handle = new_feed.spawn(tx.clone());

                            logger.window_start(&market.slug);
                        }
                        Err(e) => {
                            println!(">>> Failed to fetch current market: {}", e);
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                    continue;
                }

                // Check halt conditions
                let in_warmup = market_age < WARMUP_SECS;
                let in_cooldown = btc_guard.in_cooldown(now);
                let now_ms = (now * 1000.0) as i64;
                let is_stale = now_ms - book.last_update_ms > STALE_MS;

                // Need book data
                if !book.is_synced() {
                    continue;
                }

                // Compute mid from best bid/ask
                let yes_bid = book.best_bid(Side::Yes).unwrap();
                let yes_ask = book.best_ask(Side::Yes).unwrap();
                let mid = (yes_bid + yes_ask) as f64 / 2.0 / 1000.0;
                let mid_valid = Quotes::should_quote(mid);

                // If any halt condition, cancel all and skip
                if in_warmup || !mid_valid || in_cooldown || is_stale {
                    window_stats.ticks_halted += 1;
                    if orders.total_count() > 0 {
                        let reason = if in_warmup {
                            "WARMUP"
                        } else if !mid_valid {
                            "MID_RANGE"
                        } else if in_cooldown {
                            "BTC_COOLDOWN"
                        } else {
                            window_stats.stale_halts += 1;
                            "STALE_BOOK"
                        };
                        let cancelled = orders.total_count();
                        logger.halt(time_left, &market.slug, mid, var_est.current_var(), reason, cancelled);
                        let actions = vec![Action::CancelAll];
                        let _ = executor.execute(actions, &mut orders).await;
                        session_stats.orders_cancelled += cancelled as u32;
                    }
                    continue;
                }

                // Compute A-S quotes
                let inventory = position.net_position().to_string().parse::<f64>().unwrap_or(0.0);
                let var = var_est.current_var();
                let k = flow_est.current_k();

                let quotes = as_pricer.compute_quotes(mid, inventory, var, k, time_left);

                // Convert to ticks and round to cents
                let yes_target = round_to_cents(AvellanedaStoikov::to_ticks(quotes.yes_bid));
                let no_target = round_to_cents(AvellanedaStoikov::to_ticks(quotes.no_bid));

                // Track that we're quoting this tick
                window_stats.ticks_quoted += 1;

                // Get current resting prices
                let yes_resting = orders.top_price(Side::Yes).unwrap_or(0);
                let no_resting = orders.top_price(Side::No).unwrap_or(0);

                // Log tick to CSV (always) and stdout (throttled)
                logger.tick(
                    time_left,
                    &market.slug,
                    mid,
                    var,
                    k,
                    inventory,
                    yes_target,
                    no_target,
                    yes_resting,
                    no_resting,
                );

                // Reconcile orders
                let mut actions = Vec::new();

                // YES side
                let old_yes = yes_resting;
                reconcile_side(Side::Yes, yes_target, &orders, &mut actions);

                // NO side
                let old_no = no_resting;
                reconcile_side(Side::No, no_target, &orders, &mut actions);

                // Log price replacements
                if old_yes > 0 && yes_target != old_yes {
                    logger.replace(Side::Yes, old_yes, yes_target);
                }
                if old_no > 0 && no_target != old_no {
                    logger.replace(Side::No, old_no, no_target);
                }

                // Count stats for actions about to execute
                for action in &actions {
                    match action {
                        Action::Place { .. } => {
                            session_stats.orders_placed += 1;
                        }
                        Action::Cancel { order_id } => {
                            session_stats.orders_cancelled += 1;
                            logger.cancel(time_left, &market.slug, order_id, "REPLACE");
                        }
                        Action::CancelAll => {
                            session_stats.orders_cancelled += orders.total_count() as u32;
                        }
                        _ => {}
                    }
                }

                // Execute actions (executor logs individual orders internally via tracing)
                if !actions.is_empty() {
                    if let Err(e) = executor.execute(actions, &mut orders).await {
                        session_stats.order_fails += 1;
                        println!("[ERROR] Executor failed: {}", e);
                    }
                }
            }

            // Process events
            Some(event) = rx.recv() => {
                let now = now_secs();

                let time_left = market_end - now;

                match event {
                    Event::BtcPrice { price } => {
                        let old_price = last_btc_price;
                        last_btc_price = price;
                        // Check for crash
                        if btc_guard.update(price, now) {
                            let cancelled = orders.total_count();
                            window_stats.toxic_cancels += 1;
                            session_stats.toxic_cancels += 1;
                            logger.toxic(time_left, &market.slug, old_price, price, BTC_WINDOW_SECS, cancelled);
                            if cancelled > 0 {
                                session_stats.orders_cancelled += cancelled as u32;
                                let actions = vec![Action::CancelAll];
                                let _ = executor.execute(actions, &mut orders).await;
                            }
                        }
                    }

                    Event::BookUpdate { side, bid, ask } => {
                        book.update(side, bid, ask, (now * 1000.0) as i64);

                        // Update variance estimator with YES mid
                        if side == Side::Yes && bid > 0 && ask > 0 {
                            let mid = (bid + ask) as f64 / 2.0 / 1000.0;
                            var_est.update_poly(mid, now);
                        }
                    }

                    Event::Trade { .. } => {
                        flow_est.record_trade(now);
                    }

                    Event::OrderFill { order_id, side, price, size, is_maker } => {
                        // Update position
                        let size_dec = Decimal::try_from(size).unwrap_or(dec!(0));
                        position.apply_fill(side, price, size_dec);

                        // Remove from order tracker
                        orders.update_fill(side, &order_id, size_dec);

                        // Record fill for FIFO matching and pair cost calculation
                        let old_matched = window_stats.matched_pairs;
                        window_stats.record_fill(side, price);

                        // Get pair cost if a new pair was matched
                        let pair_cost = if window_stats.matched_pairs > old_matched {
                            window_stats.pair_costs.last().copied()
                        } else {
                            None
                        };

                        // Log fill
                        let inv_yes = position.qty_yes.to_string().parse::<f64>().unwrap_or(0.0);
                        let inv_no = position.qty_no.to_string().parse::<f64>().unwrap_or(0.0);
                        logger.fill(
                            time_left,
                            &market.slug,
                            side,
                            price,
                            size,
                            &order_id,
                            is_maker,
                            inv_yes,
                            inv_no,
                            pair_cost,
                            window_stats.gross_pnl,
                        );
                        // Next tick will see missing order via OrderTracker and place new one
                    }

                    Event::Shutdown => {
                        if orders.total_count() > 0 {
                            session_stats.orders_cancelled += orders.total_count() as u32;
                            let actions = vec![Action::CancelAll];
                            let _ = executor.execute(actions, &mut orders).await;
                        }
                        break;
                    }

                    Event::Tick => {}
                }
            }

            // Ctrl+C
            _ = tokio::signal::ctrl_c() => {
                if orders.total_count() > 0 {
                    session_stats.orders_cancelled += orders.total_count() as u32;
                    let actions = vec![Action::CancelAll];
                    let _ = executor.execute(actions, &mut orders).await;
                }
                break;
            }
        }
    }

    // Finalize current window and merge into session
    window_stats.finalize();
    let yes_shares = position.qty_yes.to_string().parse::<f64>().unwrap_or(0.0);
    let yes_avg = position.avg_price_yes().map(|p| p.to_string().parse::<f64>().unwrap_or(0.0) / 10.0).unwrap_or(0.0);
    let no_shares = position.qty_no.to_string().parse::<f64>().unwrap_or(0.0);
    let no_avg = position.avg_price_no().map(|p| p.to_string().parse::<f64>().unwrap_or(0.0) / 10.0).unwrap_or(0.0);
    let min_pnl = position.min_pnl_usd().to_string().parse::<f64>().unwrap_or(0.0);
    logger.window_end(
        &market.slug,
        &window_stats,
        yes_shares,
        yes_avg,
        no_shares,
        no_avg,
        min_pnl,
        AS_GAMMA,
        HALT_SECS,
    );
    session_stats.merge_window(&window_stats);

    // Log session summary
    logger.session_summary(&session_stats);
    logger.flush();

    Ok(())
}

/// Reconcile a single side: cancel if price changed, place if missing.
/// Uses OrderTracker to get actual resting price (no separate tracking).
fn reconcile_side(
    side: Side,
    target_price: u16,
    orders: &OrderTracker,
    actions: &mut Vec<Action>,
) {
    let resting_price = orders.top_price(side).unwrap_or(0);
    let has_order = resting_price > 0;
    let price_changed = has_order && target_price != resting_price;

    // If price changed and we have orders, cancel them
    if price_changed {
        for order in orders.all_orders(side) {
            actions.push(Action::Cancel {
                order_id: order.order_id.clone(),
            });
        }
    }

    // If no order (or just cancelled), place new one
    if !has_order || price_changed {
        actions.push(Action::Place {
            side,
            price: target_price,
            size: Decimal::from(ORDER_SIZE),
        });
    }
}
