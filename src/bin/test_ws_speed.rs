use futures_util::StreamExt;
use tokio_tungstenite::connect_async;
use std::time::Instant;

const BINANCE_WS: &str = "wss://stream.binance.com:9443/ws/btcusdt@trade";

#[derive(serde::Deserialize)]
struct Trade {
    p: String,  // price
}

#[tokio::main]
async fn main() {
    println!("Connecting to Binance...");
    let (ws, _) = connect_async(BINANCE_WS).await.expect("Failed to connect");
    let (_, mut read) = ws.split();

    println!("Connected. Measuring parse time for 100 messages...\n");

    let mut times: Vec<u128> = Vec::new();
    let mut count = 0;

    while let Some(msg) = read.next().await {
        if let Ok(tungstenite::Message::Text(text)) = msg {
            let start = Instant::now();

            // Parse JSON and extract price (using simd-json)
            let mut bytes = text.into_bytes();
            if let Ok(trade) = simd_json::from_slice::<Trade>(&mut bytes) {
                let _price: f64 = trade.p.parse().unwrap_or(0.0);
                let elapsed = start.elapsed().as_nanos();
                times.push(elapsed);
                count += 1;

                if count <= 5 || count % 20 == 0 {
                    println!("#{}: {}ns ({}μs)", count, elapsed, elapsed / 1000);
                }

                if count >= 100 {
                    break;
                }
            }
        }
    }

    // Stats
    times.sort();
    let sum: u128 = times.iter().sum();
    let avg = sum / times.len() as u128;
    let min = times[0];
    let max = times[times.len() - 1];
    let median = times[times.len() / 2];
    let p99 = times[99];

    println!("\n=== RUST WS PARSE LATENCY (100 messages) ===");
    println!("Min:    {}ns ({}μs)", min, min / 1000);
    println!("Max:    {}ns ({}μs)", max, max / 1000);
    println!("Avg:    {}ns ({}μs)", avg, avg / 1000);
    println!("Median: {}ns ({}μs)", median, median / 1000);
    println!("P99:    {}ns ({}μs)", p99, p99 / 1000);
}
