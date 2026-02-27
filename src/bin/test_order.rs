use anyhow::Result;
use polyfill_rs::{ClobClient, OrderArgs, OrderType, Side};
use polyfill_rs::orders::SigType;
use polyfill_rs::types::ExtraOrderArgs;
use alloy_primitives::{Address, U256};
use rust_decimal_macros::dec;
use std::str::FromStr;
use std::time::Instant;

use polybot_rs::api::gamma;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let private_key = std::env::var("POLY_PRIVATE_KEY")?;
    let proxy_wallet = std::env::var("POLY_PROXY_WALLET")?;
    let funder = Address::from_str(&proxy_wallet)?;

    // Create L1 client and derive API keys
    println!("Creating client...");
    let l1_client = ClobClient::with_l1_headers(
        "https://clob.polymarket.com",
        &private_key,
        137,
    );
    let api_creds = l1_client.create_or_derive_api_key(None).await?;

    // Create L2 client with proxy wallet
    let client = ClobClient::with_l2_headers(
        "https://clob.polymarket.com",
        &private_key,
        137,
        api_creds,
        Some(SigType::PolyProxy),
        Some(funder),
    );

    // Get current market
    let market = gamma::get_current_15m_market().await?;
    println!("Market: {} | Token: {}...", market.slug, &market.yes_token[..20]);

    // Place order
    let args = OrderArgs::new(&market.yes_token, dec!(0.01), dec!(5.0), Side::BUY);
    let extras = ExtraOrderArgs {
        fee_rate_bps: 1000,
        nonce: U256::ZERO,
        taker: "0x0000000000000000000000000000000000000000".to_string(),
    };

    let start = Instant::now();
    let order = client.create_order(&args, None, Some(extras), None).await?;
    let response = client.post_order(order, OrderType::GTC).await?;
    let place_ms = start.elapsed().as_millis();

    // Cancel order
    let start = Instant::now();
    client.cancel(&response.order_id).await?;
    let cancel_ms = start.elapsed().as_millis();

    println!("Place: {}ms | Cancel: {}ms", place_ms, cancel_ms);

    Ok(())
}
