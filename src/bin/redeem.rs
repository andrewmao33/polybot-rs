//! Redeem all resolved positions and merge paired shares to free up USDC.
//!
//! Usage:
//!     cargo run --bin redeem
//!
//! Required env vars:
//!     POLY_PRIVATE_KEY=0x...
//!     POLY_PROXY_WALLET=0x...

use std::str::FromStr;

use alloy::primitives::{B256, U256};
use alloy::providers::ProviderBuilder;
use alloy::signers::Signer as _;
use alloy::signers::local::LocalSigner;
use anyhow::Result;
use polymarket_client_sdk::ctf;
use polymarket_client_sdk::ctf::types::{MergePositionsRequest, RedeemPositionsRequest};
use polymarket_client_sdk::types::address;

fn polygon_rpc() -> String {
    std::env::var("POLYGON_RPC_URL").unwrap_or_else(|_| "https://polygon-rpc.com".to_string())
}
const POLYGON_CHAIN_ID: u64 = 137;
const USDC: alloy::primitives::Address = address!("0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174");

// Polymarket uses 6-decimal USDC. Shares are in the same units.
// 1 share = 1_000_000 units (same as 1 USDC).

#[derive(serde::Deserialize, Debug)]
struct Position {
    #[serde(rename = "conditionId")]
    condition_id: Option<String>,
    asset: Option<String>,
    size: Option<String>,
    #[serde(rename = "avgPrice")]
    avg_price: Option<String>,
    #[serde(rename = "curPrice")]
    cur_price: Option<String>,
}

#[derive(Debug)]
struct MarketPosition {
    condition_id: String,
    yes_size: f64,
    no_size: f64,
    resolved: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let private_key = std::env::var("POLY_PRIVATE_KEY").expect("Set POLY_PRIVATE_KEY");
    let proxy_wallet = std::env::var("POLY_PROXY_WALLET").expect("Set POLY_PROXY_WALLET");

    println!("=== Polymarket Position Redeemer ===");
    println!("Wallet: {}", proxy_wallet);

    // Fetch all positions
    let http = reqwest::Client::new();
    let positions: Vec<Position> = http
        .get("https://data-api.polymarket.com/positions")
        .query(&[("user", &proxy_wallet), ("sizeThreshold", &"0".to_string())])
        .send()
        .await?
        .json()
        .await?;

    println!("Found {} position entries", positions.len());

    // Group by condition_id
    let mut markets: std::collections::HashMap<String, MarketPosition> =
        std::collections::HashMap::new();

    for pos in &positions {
        let cid = match &pos.condition_id {
            Some(c) => c.clone(),
            None => continue,
        };
        let size: f64 = pos
            .size
            .as_deref()
            .unwrap_or("0")
            .parse()
            .unwrap_or(0.0);
        let cur_price: f64 = pos
            .cur_price
            .as_deref()
            .unwrap_or("0.5")
            .parse()
            .unwrap_or(0.5);

        if size <= 0.0 {
            continue;
        }

        // Resolved markets have cur_price of exactly 0 or 1
        let resolved = cur_price == 0.0 || cur_price == 1.0;

        let entry = markets.entry(cid.clone()).or_insert(MarketPosition {
            condition_id: cid,
            yes_size: 0.0,
            no_size: 0.0,
            resolved,
        });

        // First token listed is YES, second is NO
        // We detect by price: if cur_price >= 0.5 it's likely the winning/YES side
        // But actually we don't know which asset is YES vs NO from this API alone.
        // We'll just track total per side and merge min(yes, no) regardless.
        if entry.yes_size == 0.0 {
            entry.yes_size = size;
        } else {
            entry.no_size = size;
        }
        entry.resolved = entry.resolved || resolved;
    }

    // Filter to markets we can act on
    let redeemable: Vec<&MarketPosition> = markets
        .values()
        .filter(|m| m.resolved || (m.yes_size > 0.0 && m.no_size > 0.0))
        .collect();

    if redeemable.is_empty() {
        println!("\nNo positions to redeem or merge.");
        return Ok(());
    }

    println!("\n--- Actionable Markets ---");
    for m in &redeemable {
        let action = if m.resolved {
            "REDEEM"
        } else if m.yes_size > 0.0 && m.no_size > 0.0 {
            "MERGE"
        } else {
            continue;
        };
        println!(
            "  {} | cid={}... | yes={:.1} no={:.1}",
            action,
            &m.condition_id[..20.min(m.condition_id.len())],
            m.yes_size,
            m.no_size,
        );
    }

    // Set up on-chain client
    let signer = LocalSigner::from_str(&private_key)?.with_chain_id(Some(POLYGON_CHAIN_ID));
    let rpc = polygon_rpc();
    let provider = ProviderBuilder::new()
        .wallet(signer.clone())
        .connect(&rpc)
        .await?;
    let ctf_client = ctf::Client::new(provider, POLYGON_CHAIN_ID)?;

    println!("\n--- Executing ---");

    for m in &redeemable {
        let cid = B256::from_str(&m.condition_id)?;

        if m.resolved {
            // Redeem winning tokens
            println!("REDEEM {}...", &m.condition_id[..20.min(m.condition_id.len())]);
            let req = RedeemPositionsRequest::for_binary_market(USDC, cid);
            match ctf_client.redeem_positions(&req).await {
                Ok(resp) => println!("  OK tx={}", resp.transaction_hash),
                Err(e) => println!("  FAILED: {}", e),
            }
        }

        if m.yes_size > 0.0 && m.no_size > 0.0 {
            // Merge paired shares
            let merge_qty = m.yes_size.min(m.no_size);
            // Convert shares to on-chain units (6 decimals for USDC)
            let amount = U256::from((merge_qty * 1_000_000.0) as u64);
            println!(
                "MERGE {}... ({:.1} pairs = ${:.2})",
                &m.condition_id[..20.min(m.condition_id.len())],
                merge_qty,
                merge_qty, // 1 pair = $1.00
            );
            let req = MergePositionsRequest::for_binary_market(USDC, cid, amount);
            match ctf_client.merge_positions(&req).await {
                Ok(resp) => println!("  OK tx={}", resp.transaction_hash),
                Err(e) => println!("  FAILED: {}", e),
            }
        }
    }

    println!("\nDone.");
    Ok(())
}
