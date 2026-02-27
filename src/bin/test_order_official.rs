use anyhow::Result;
use polymarket_client_sdk::clob::{Client, Config};
use polymarket_client_sdk::clob::types::{Side, SignatureType};
use polymarket_client_sdk::auth::Signer;
use alloy::signers::local::LocalSigner;
use alloy_primitives::{U256, Address};
use rust_decimal_macros::dec;
use std::time::Instant;
use std::str::FromStr;

use polybot_rs::api::gamma;

const POLYGON: u64 = 137;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let private_key = std::env::var("POLY_PRIVATE_KEY")?;
    let proxy_wallet = std::env::var("POLY_PROXY_WALLET")?;
    let funder = Address::from_str(&proxy_wallet)?;

    // Create signer and authenticate with proxy wallet
    println!("Creating client...");
    let signer = LocalSigner::from_str(&private_key)?
        .with_chain_id(Some(POLYGON));

    let client = Client::new("https://clob.polymarket.com", Config::default())?
        .authentication_builder(&signer)
        .funder(funder)
        .signature_type(SignatureType::Proxy)
        .authenticate()
        .await?;

    // Get current market
    let market = gamma::get_current_15m_market().await?;
    println!("Market: {} | Token: {}...", market.slug, &market.yes_token[..20]);

    // Parse token ID to U256
    let token_id = U256::from_str(&market.yes_token)?;

    // Place order
    let start = Instant::now();
    let order = client
        .limit_order()
        .token_id(token_id)
        .size(dec!(5.0))
        .price(dec!(0.01))
        .side(Side::Buy)
        .build()
        .await?;
    let signed_order = client.sign(&signer, order).await?;
    let response = client.post_order(signed_order).await?;
    let place_ms = start.elapsed().as_millis();

    // Cancel order
    let start = Instant::now();
    client.cancel_order(&response.order_id).await?;
    let cancel_ms = start.elapsed().as_millis();

    println!("Place: {}ms | Cancel: {}ms", place_ms, cancel_ms);

    Ok(())
}
