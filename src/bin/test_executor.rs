//! Test Executor: place and cancel orders via the executor.
//!
//! Usage:
//!     cargo run --bin test_executor
//!
//! Required env vars:
//!     POLY_PRIVATE_KEY=0x...
//!     POLY_PROXY_WALLET=0x...

use anyhow::Result;
use alloy_primitives::Address;
use polyfill_rs::ClobClient;
use polyfill_rs::orders::SigType;
use rust_decimal_macros::dec;
use std::str::FromStr;

use polybot_rs::api::gamma;
use polybot_rs::events::Side;
use polybot_rs::executor::{Executor, ExecutorConfig};
use polybot_rs::state::OrderTracker;
use polybot_rs::strategy::Action;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let private_key = std::env::var("POLY_PRIVATE_KEY")
        .expect("Set POLY_PRIVATE_KEY");
    let proxy_wallet = std::env::var("POLY_PROXY_WALLET")
        .expect("Set POLY_PROXY_WALLET");
    let funder = Address::from_str(&proxy_wallet)?;

    println!("=== Executor Test ===\n");

    // Create CLOB client
    println!("Creating CLOB client...");
    let l1_client = ClobClient::with_l1_headers(
        "https://clob.polymarket.com",
        &private_key,
        137,
    );
    let api_creds = l1_client.create_or_derive_api_key(None).await?;

    let client = ClobClient::with_l2_headers(
        "https://clob.polymarket.com",
        &private_key,
        137,
        api_creds,
        Some(SigType::PolyProxy),
        Some(funder),
    );

    // Get current market
    println!("Fetching current market...");
    let market = gamma::get_current_15m_market().await?;
    println!("Market: {}", market.slug);
    println!("YES: {}...", &market.yes_token[..20]);
    println!("NO: {}...", &market.no_token[..20]);

    // Create executor
    let config = ExecutorConfig {
        log_only: false,
        yes_token: market.yes_token.clone(),
        no_token: market.no_token.clone(),
    };
    let executor = Executor::new(client, config);

    // Create order tracker
    let mut orders = OrderTracker::new();

    println!("\n--- Test 1: Place YES order (5 shares @ 1¢) ---");
    let actions = vec![
        Action::place(Side::Yes, 10, dec!(5)), // 10 ticks = 1 cent
    ];
    executor.execute(actions, &mut orders).await?;

    println!("OrderTracker YES count: {}", orders.count(Side::Yes));
    println!("OrderTracker YES orders: {:?}", orders.all_order_ids(Side::Yes));

    // Wait a moment
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    println!("\n--- Test 2: Place NO order (5 shares @ 1¢) ---");
    let actions = vec![
        Action::place(Side::No, 10, dec!(5)),
    ];
    executor.execute(actions, &mut orders).await?;

    println!("OrderTracker NO count: {}", orders.count(Side::No));
    println!("OrderTracker total: {}", orders.total_count());

    // Wait a moment
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    println!("\n--- Test 3: Cancel all ---");
    let actions = vec![Action::cancel_all()];
    executor.execute(actions, &mut orders).await?;

    println!("OrderTracker total after cancel: {}", orders.total_count());

    println!("\n=== Done ===");
    Ok(())
}
