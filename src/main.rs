mod api;
mod config;
mod events;
mod feeds;

use api::gamma;
use config::Config;
use events::{Event, Side};
use feeds::binance;
use feeds::polymarket::PolymarketFeed;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() {
    let cfg = Config::load("config.toml").expect("Failed to load config");
    println!("Loaded config: {:?}", cfg);

    // Fetch current market from Gamma API
    println!("Fetching current BTC 5m market...");
    let market = gamma::get_current_5m_market()
        .await
        .expect("Failed to fetch market");

    println!("Market: {}", market.slug);
    println!("YES token: {}", market.yes_token);
    println!("NO token: {}", market.no_token);

    // Create the event channel
    let (tx, mut rx) = mpsc::channel::<Event>(100);

    // Start feeds
    let poly_feed = PolymarketFeed::new(market.yes_token, market.no_token);
    poly_feed.spawn(tx.clone());
    binance::spawn(tx.clone());

    // Main event loop
    println!("\nStarting event loop... (Ctrl+C to quit)\n");
    while let Some(event) = rx.recv().await {
        match event {
            Event::BtcPrice { price } => {
                println!("BTC: ${:.2}", price);
            }
            Event::BookUpdate { side, bid, ask } => {
                let side_str = match side {
                    Side::Yes => "YES",
                    Side::No => "NO ",
                };
                let b = bid as f64 / 1000.0;
                let a = ask as f64 / 1000.0;
                println!("{} bid:{:.2} ask:{:.2}", side_str, b, a);
            }
            Event::Shutdown => {
                println!("Shutting down...");
                break;
            }
            _ => {}
        }
    }
}
