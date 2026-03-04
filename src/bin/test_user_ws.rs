//! Test User WebSocket fill notifications.
//!
//! Connects to the current BTC market and prints fill events.
//! Manually place trades to verify fills are received.
//!
//! Usage:
//!     cargo run --bin test_user_ws
//!
//! Required env vars (in .env or exported):
//!     POLY_PRIVATE_KEY=0x...
//!     POLY_PROXY_WALLET=0x...

use anyhow::Result;
use polyfill_rs::ClobClient;
use tokio::sync::mpsc;

use polybot_rs::api::gamma;
use polybot_rs::events::Event;
use polybot_rs::feeds::user_ws::{UserFeed, UserFeedConfig};

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let private_key = std::env::var("POLY_PRIVATE_KEY")
        .expect("Set POLY_PRIVATE_KEY environment variable");
    let proxy_wallet = std::env::var("POLY_PROXY_WALLET")
        .expect("Set POLY_PROXY_WALLET environment variable");

    println!("=== User WebSocket Test ===\n");

    // Create L1 client and derive API keys
    println!("Creating CLOB client and deriving API keys...");
    let l1_client = ClobClient::with_l1_headers(
        "https://clob.polymarket.com",
        &private_key,
        137,
    );
    let api_creds = l1_client.create_or_derive_api_key(None).await?;

    println!("API Key: {}...", &api_creds.api_key[..20]);
    println!("Maker Address: {}", proxy_wallet);

    // Get current market
    println!("\nFetching current BTC 15min market...");
    let market = gamma::get_current_15m_market().await?;
    println!("Market: {}", market.slug);
    println!("YES token: {}...", &market.yes_token[..20]);
    println!("NO token: {}...", &market.no_token[..20]);

    // Create event channel
    let (tx, mut rx) = mpsc::channel::<Event>(100);

    // Create and spawn UserFeed
    println!("\n{}", "=".repeat(60));
    println!("Starting User WebSocket...");
    println!("{}", "=".repeat(60));

    let config = UserFeedConfig {
        api_key: api_creds.api_key,
        api_secret: api_creds.secret,
        api_passphrase: api_creds.passphrase,
        maker_address: proxy_wallet,
        yes_token: market.yes_token,
        no_token: market.no_token,
    };

    let feed = UserFeed::new(config);
    feed.spawn(tx);

    println!("\nListening for fills. Place a trade manually to test.");
    println!("Press Ctrl+C to stop.\n");

    // Listen for fill events
    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                match event {
                    Event::OrderFill { order_id, side, price, size, is_maker } => {
                        println!("\n{}", "=".repeat(60));
                        println!(">>> FILL RECEIVED!");
                        println!("    Type: {}", if is_maker { "MAKER" } else { "TAKER" });
                        println!("    Side: {:?}", side);
                        println!("    Size: {:.1} shares", size);
                        println!("    Price: {} ticks (${:.2})", price, price as f64 / 1000.0);
                        println!("    Order: {}...", &order_id[..order_id.len().min(20)]);
                        println!("{}", "=".repeat(60));
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
