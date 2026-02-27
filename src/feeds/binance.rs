use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;

use crate::events::Event;

const BINANCE_WS_URL: &str = "wss://stream.binance.com:9443/ws/btcusdt@trade";

// Binance sends this JSON shape for each trade
#[derive(serde::Deserialize)]
struct BinanceTrade {
    #[serde(rename = "p")]
    price: String,
}

/// Spawns a task that connects to Binance and sends BtcPrice events
pub fn spawn(tx: mpsc::Sender<Event>) {
    tokio::spawn(async move {
        loop {
            println!("[binance] Connecting...");

            match connect_async(BINANCE_WS_URL).await {
                Ok((ws_stream, _)) => {
                    println!("[binance] Connected!");

                    let (_, mut read) = ws_stream.split();

                    while let Some(msg) = read.next().await {
                        match msg {
                            Ok(tungstenite::Message::Text(text)) => {
                                let mut bytes = text.into_bytes();
                                if let Ok(trade) = simd_json::from_slice::<BinanceTrade>(&mut bytes) {
                                    if let Ok(price) = trade.price.parse::<f64>() {
                                        let _ = tx.send(Event::BtcPrice { price }).await;
                                    }
                                }
                            }
                            Err(e) => {
                                println!("[binance] Error: {}", e);
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    println!("[binance] Failed to connect: {}", e);
                }
            }

            // Wait before reconnecting
            println!("[binance] Reconnecting in 5 seconds...");
            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
        }
    });
}
