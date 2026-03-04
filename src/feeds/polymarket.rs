use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;

use crate::events::{Event, Side};

const POLYMARKET_WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";

// Message we send to subscribe
#[derive(serde::Serialize)]
struct SubscribeMsg {
    assets_ids: Vec<String>,
    operation: String,
    custom_feature_enabled: bool,
}

// Generic message shape - Polymarket sends various event types
#[derive(serde::Deserialize, Debug)]
struct PolyMessage {
    event_type: Option<String>,
    asset_id: Option<String>,
    // For best_bid_ask
    best_bid: Option<String>,
    best_ask: Option<String>,
    // For last_trade_price
    price: Option<String>,
}

pub struct PolymarketFeed {
    yes_token: String,
    no_token: String,
}

impl PolymarketFeed {
    pub fn new(yes_token: String, no_token: String) -> Self {
        Self { yes_token, no_token }
    }

    /// Spawns a task that connects and sends BookUpdate events.
    /// Returns a JoinHandle that can be aborted to stop the feed.
    pub fn spawn(self, tx: mpsc::Sender<Event>) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                println!("[polymarket] Connecting...");

                match connect_async(POLYMARKET_WS_URL).await {
                    Ok((ws_stream, _)) => {
                        println!("[polymarket] Connected!");

                        let (mut write, mut read) = ws_stream.split();

                        // Subscribe to both tokens
                        let subscribe = SubscribeMsg {
                            assets_ids: vec![self.yes_token.clone(), self.no_token.clone()],
                            operation: "subscribe".to_string(),
                            custom_feature_enabled: true,
                        };

                        let msg = serde_json::to_string(&subscribe).unwrap();
                        if let Err(e) = write.send(tungstenite::Message::Text(msg)).await {
                            println!("[polymarket] Failed to subscribe: {}", e);
                            continue;
                        }

                        println!("[polymarket] Subscribed to tokens");

                        // Track last prices to detect changes
                        let mut last_yes: (u16, u16) = (0, 0);
                        let mut last_no: (u16, u16) = (0, 0);

                        while let Some(msg) = read.next().await {
                            match msg {
                                Ok(tungstenite::Message::Text(text)) => {
                                    if let Ok(msg) = serde_json::from_str::<PolyMessage>(&text) {
                                        let asset_id = msg.asset_id.as_deref().unwrap_or("");

                                        // Determine which side
                                        let side = if asset_id == self.yes_token {
                                            Some(Side::Yes)
                                        } else if asset_id == self.no_token {
                                            Some(Side::No)
                                        } else {
                                            None
                                        };

                                        match msg.event_type.as_deref() {
                                            Some("best_bid_ask") => {
                                                // Parse prices
                                                let bid = msg.best_bid
                                                    .as_ref()
                                                    .and_then(|s| s.parse::<f64>().ok())
                                                    .map(|p| (p * 1000.0) as u16);

                                                let ask = msg.best_ask
                                                    .as_ref()
                                                    .and_then(|s| s.parse::<f64>().ok())
                                                    .map(|p| (p * 1000.0) as u16);

                                                // Only send if changed
                                                if let (Some(s), Some(b), Some(a)) = (side, bid, ask) {
                                                    let changed = match s {
                                                        Side::Yes => {
                                                            let c = (b, a) != last_yes;
                                                            last_yes = (b, a);
                                                            c
                                                        }
                                                        Side::No => {
                                                            let c = (b, a) != last_no;
                                                            last_no = (b, a);
                                                            c
                                                        }
                                                    };

                                                    if changed {
                                                        let _ = tx.send(Event::BookUpdate {
                                                            side: s,
                                                            bid: b,
                                                            ask: a,
                                                        }).await;
                                                    }
                                                }
                                            }
                                            Some("last_trade_price") => {
                                                // Trade event - for flow estimator (k)
                                                if let Some(s) = side {
                                                    let price = msg.price
                                                        .as_ref()
                                                        .and_then(|p| p.parse::<f64>().ok())
                                                        .map(|p| (p * 1000.0) as u16)
                                                        .unwrap_or(0);

                                                    let _ = tx.send(Event::Trade {
                                                        side: s,
                                                        price,
                                                    }).await;
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                Err(e) => {
                                    println!("[polymarket] Error: {}", e);
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        println!("[polymarket] Failed to connect: {}", e);
                    }
                }

                println!("[polymarket] Reconnecting in 5 seconds...");
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        })
    }
}
