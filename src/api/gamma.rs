use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

const GAMMA_BASE: &str = "https://gamma-api.polymarket.com";

/// Market data from Gamma API
#[derive(Debug, Deserialize)]
pub struct Market {
    #[serde(rename = "conditionId")]
    pub condition_id: String,

    #[serde(rename = "clobTokenIds")]
    pub clob_token_ids: String, // JSON string like "[\"abc\", \"def\"]"

    #[serde(rename = "endDate")]
    pub end_date: Option<String>,

    pub slug: Option<String>,
}

/// Parsed market info with extracted token IDs
#[derive(Debug)]
pub struct MarketInfo {
    pub condition_id: String,
    pub yes_token: String,
    pub no_token: String,
    pub end_date: Option<String>,
    pub slug: String,
}

/// Get current unix timestamp
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Floor timestamp to 15-minute boundary
fn floor_15m(ts: u64) -> u64 {
    ts - (ts % 900)
}

/// Floor timestamp to 5-minute boundary
fn floor_5m(ts: u64) -> u64 {
    ts - (ts % 300)
}

/// Fetch current BTC 15-minute market
pub async fn get_current_15m_market() -> Result<MarketInfo> {
    let epoch = floor_15m(now());
    let slug = format!("btc-updown-15m-{}", epoch);
    fetch_market_by_slug(&slug).await
}

/// Fetch current BTC 5-minute market
pub async fn get_current_5m_market() -> Result<MarketInfo> {
    let epoch = floor_5m(now());
    let slug = format!("btc-updown-5m-{}", epoch);
    fetch_market_by_slug(&slug).await
}

/// Fetch market by slug from Gamma API
async fn fetch_market_by_slug(slug: &str) -> Result<MarketInfo> {
    let url = format!("{}/markets/slug/{}", GAMMA_BASE, slug);

    let client = reqwest::Client::new();
    let response = client.get(&url).send().await?;

    if response.status() == 404 {
        return Err(anyhow!("Market not found: {}", slug));
    }

    let market: Market = response.json().await?;

    // Parse the clob_token_ids JSON string into a Vec
    let token_ids: Vec<String> = serde_json::from_str(&market.clob_token_ids)?;

    if token_ids.len() < 2 {
        return Err(anyhow!("Market {} has less than 2 tokens", slug));
    }

    Ok(MarketInfo {
        condition_id: market.condition_id,
        yes_token: token_ids[0].clone(),
        no_token: token_ids[1].clone(),
        end_date: market.end_date,
        slug: slug.to_string(),
    })
}
