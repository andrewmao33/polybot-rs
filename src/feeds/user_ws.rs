//! Polymarket User Channel WebSocket handler.
//! Receives real-time fill notifications with actual execution prices.
//!
//! Mirrors the Python implementation in polybot/ingestion/user_ws.py

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;

use crate::events::{Event, Side};

const USER_WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/user";

/// Authentication message sent on connect
#[derive(serde::Serialize)]
struct AuthMessage {
    #[serde(rename = "type")]
    msg_type: String,
    auth: AuthCredentials,
}

#[derive(serde::Serialize)]
struct AuthCredentials {
    #[serde(rename = "apiKey")]
    api_key: String,
    secret: String,
    passphrase: String,
}

/// Trade event from the user channel
#[derive(serde::Deserialize, Debug)]
struct TradeEvent {
    event_type: Option<String>,
    status: Option<String>,
    trader_side: Option<String>,
    // For taker fills
    taker_order_id: Option<String>,
    asset_id: Option<String>,
    price: Option<String>,
    size: Option<String>,
    market: Option<String>,
    timestamp: Option<String>,
    // For maker fills
    maker_orders: Option<Vec<MakerOrder>>,
}

#[derive(serde::Deserialize, Debug)]
struct MakerOrder {
    order_id: Option<String>,
    asset_id: Option<String>,
    maker_address: Option<String>,
    price: Option<String>,
    matched_amount: Option<String>,
}

/// Configuration for the user WebSocket feed
pub struct UserFeedConfig {
    pub api_key: String,
    pub api_secret: String,
    pub api_passphrase: String,
    pub maker_address: String,
    pub yes_token: String,
    pub no_token: String,
}

pub struct UserFeed {
    config: UserFeedConfig,
}

impl UserFeed {
    pub fn new(config: UserFeedConfig) -> Self {
        Self { config }
    }

    /// Spawns a task that connects and sends OrderFill events.
    /// Returns a JoinHandle that can be aborted on market switch.
    pub fn spawn(self, tx: mpsc::Sender<Event>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut reconnect_delay = 1u64;
            let max_reconnect_delay = 60u64;

            loop {
                println!("[user_ws] Connecting...");

                match connect_async(USER_WS_URL).await {
                    Ok((ws_stream, _)) => {
                        println!("[user_ws] Connected!");

                        let (mut write, mut read) = ws_stream.split();

                        // Send authentication message (same as Python)
                        let auth_msg = AuthMessage {
                            msg_type: "user".to_string(),
                            auth: AuthCredentials {
                                api_key: self.config.api_key.clone(),
                                secret: self.config.api_secret.clone(),
                                passphrase: self.config.api_passphrase.clone(),
                            },
                        };

                        let msg = serde_json::to_string(&auth_msg).unwrap();
                        if let Err(e) = write.send(tungstenite::Message::Text(msg)).await {
                            println!("[user_ws] Failed to authenticate: {}", e);
                            continue;
                        }

                        println!("[user_ws] Authenticated");
                        reconnect_delay = 1; // Reset on successful connect

                        // Process messages
                        while let Some(msg) = read.next().await {
                            match msg {
                                Ok(tungstenite::Message::Text(text)) => {
                                    self.process_message(&text, &tx).await;
                                }
                                Err(e) => {
                                    println!("[user_ws] Error: {}", e);
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(e) => {
                        println!("[user_ws] Failed to connect: {}", e);
                    }
                }

                println!(
                    "[user_ws] Reconnecting in {} seconds...",
                    reconnect_delay
                );
                tokio::time::sleep(tokio::time::Duration::from_secs(reconnect_delay)).await;
                reconnect_delay = (reconnect_delay * 2).min(max_reconnect_delay);
            }
        })
    }

    async fn process_message(&self, text: &str, tx: &mpsc::Sender<Event>) {
        // Try to parse as trade event
        let Ok(data) = serde_json::from_str::<TradeEvent>(text) else {
            // Not a trade event, ignore (could be heartbeat, order update, etc.)
            return;
        };

        // Only handle trade events (same as Python: event_type == "trade")
        if data.event_type.as_deref() != Some("trade") {
            return;
        }

        // Only handle MATCHED status (same as Python)
        if data.status.as_deref() != Some("MATCHED") {
            return;
        }

        let trader_side = data.trader_side.as_deref().unwrap_or("");

        match trader_side {
            "TAKER" => {
                // My order crossed the spread and filled as taker
                self.handle_taker_fill(&data, tx).await;
            }
            "MAKER" => {
                // My resting orders got matched
                self.handle_maker_fills(&data, tx).await;
            }
            _ => {}
        }
    }

    async fn handle_taker_fill(&self, data: &TradeEvent, tx: &mpsc::Sender<Event>) {
        let order_id = data.taker_order_id.clone().unwrap_or_default();
        let asset_id = data.asset_id.as_deref().unwrap_or("");

        let Some(side) = self.asset_to_side(asset_id) else {
            return;
        };

        let price = data
            .price
            .as_ref()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);

        let size = data
            .size
            .as_ref()
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);

        // Convert price to ticks (1000 = $1.00)
        let price_ticks = (price * 1000.0) as u16;

        println!(
            "[user_ws] TAKER fill: {} {:.1} @ {} ticks (order {})",
            if side == Side::Yes { "YES" } else { "NO" },
            size,
            price_ticks,
            &order_id[..order_id.len().min(20)]
        );

        let _ = tx
            .send(Event::OrderFill {
                order_id,
                side,
                price: price_ticks,
                size,
                is_maker: false,
            })
            .await;
    }

    async fn handle_maker_fills(&self, data: &TradeEvent, tx: &mpsc::Sender<Event>) {
        let Some(maker_orders) = &data.maker_orders else {
            return;
        };

        let maker_address_lower = self.config.maker_address.to_lowercase();

        for maker in maker_orders {
            // Only process fills where we are the maker (same as Python)
            let order_maker = maker.maker_address.as_deref().unwrap_or("").to_lowercase();
            if order_maker != maker_address_lower {
                continue;
            }

            let order_id = maker.order_id.clone().unwrap_or_default();
            let asset_id = maker.asset_id.as_deref().unwrap_or("");

            let Some(side) = self.asset_to_side(asset_id) else {
                continue;
            };

            let price = maker
                .price
                .as_ref()
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);

            let size = maker
                .matched_amount
                .as_ref()
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);

            // Convert price to ticks (1000 = $1.00)
            let price_ticks = (price * 1000.0) as u16;

            println!(
                "[user_ws] MAKER fill: {} {:.1} @ {} ticks (order {})",
                if side == Side::Yes { "YES" } else { "NO" },
                size,
                price_ticks,
                &order_id[..order_id.len().min(20)]
            );

            let _ = tx
                .send(Event::OrderFill {
                    order_id,
                    side,
                    price: price_ticks,
                    size,
                    is_maker: true,
                })
                .await;
        }
    }

    /// Map asset_id to Side (YES or NO)
    fn asset_to_side(&self, asset_id: &str) -> Option<Side> {
        if asset_id == self.config.yes_token {
            Some(Side::Yes)
        } else if asset_id == self.config.no_token {
            Some(Side::No)
        } else {
            None
        }
    }
}
