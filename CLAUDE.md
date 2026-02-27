# polybot-rs - Polymarket Trading Bot (Rust Rewrite)

## What This Is

Rust rewrite of [polybot](../polybot) - a market-making bot for Polymarket's BTC binary options (5-min and 15-min markets). Core strategy: buy YES and NO shares at prices summing to <$1.00 for guaranteed profit.

**Why Rust?** Lower latency to react to BTC price changes before Polymarket order book moves. The Python version only watches Polymarket; this version adds Binance BTC feed.

---

## Lessons from Python Version

### What Worked
- Profit lock (stop at guaranteed profit)
- Order stacking (add to orders without losing queue priority)
- WebSocket reconnection with backoff
- Diff engine for order reconciliation

### What Failed
1. **Fast crash exposure** - 5 rungs × 10 shares = 50 shares fill in 1 second during crashes
2. **P_mkt formula backwards** - When YES crashes, formula raises NO bid (wrong direction)
3. **No active rebalancing** - Only maker orders, no taker fills to reduce imbalance
4. **Imbalance-based sizing** - Should be time-based like Gaba
5. **Overcomplicated pricing** - Triple Gate too complex; Gaba uses simple `1.00 - ask_opposite`

### Gaba Strategy (What Works)
```
1. PLACE: 2-3 rungs, 12 shares, 1c apart, just below ask on BOTH sides
2. FOLLOW: Add rungs as market moves (don't expose wide ladder upfront)
3. REFILL: Replace filled orders within 2-24 seconds
4. REBALANCE: When imbalanced, TAKE light side (cross spread, accept loss)
5. SIZE DOWN: 12 → 6 shares as market nears end
6. RESULT: 86% maker @ $0.96 + 14% taker @ $1.02 = $0.967 overall
```

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                         MAIN PROCESS                            │
│                                                                 │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐             │
│  │   Binance   │  │ Polymarket  │  │    Timer    │             │
│  │  WS Task    │  │  WS Task    │  │    Task     │             │
│  └──────┬──────┘  └──────┬──────┘  └──────┬──────┘             │
│         │                │                │                     │
│         │    Event       │                │                     │
│         └───────────┬────┴────────────────┘                     │
│                     │                                           │
│                     ▼                                           │
│              ┌─────────────┐                                    │
│              │   Channel   │  (mpsc, bounded)                   │
│              │   (Events)  │                                    │
│              └──────┬──────┘                                    │
│                     │                                           │
│                     ▼                                           │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │                    MAIN LOOP                            │   │
│  │                                                         │   │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐     │   │
│  │  │    State    │  │  Strategy   │  │   Executor  │     │   │
│  │  │  (owned)    │◄─┤   (logic)   │──►  (orders)   │     │   │
│  │  └─────────────┘  └─────────────┘  └─────────────┘     │   │
│  │                                                         │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| Single-threaded main loop | No locks needed, deterministic event order |
| Actions, not direct API calls | Easy to test, easy to add paper trading |
| State owned by main loop | Clean separation, no shared state |
| Independent feed tasks | Each manages reconnection, isolated failures |
| Binance feed for BTC price | React before Polymarket book moves |

---

## File Structure

```
polybot-rs/
├── Cargo.toml
├── config.toml                 # Runtime configuration
├── CLAUDE.md                   # This file
├── src/
│   ├── main.rs                 # Entry point, spawns tasks, event loop
│   ├── config.rs               # Config parsing (from TOML)
│   ├── events.rs               # Event enum definition
│   │
│   ├── feeds/
│   │   ├── mod.rs
│   │   ├── binance.rs          # Binance WebSocket (BTC price)
│   │   └── polymarket.rs       # Polymarket WebSocket (book + fills)
│   │
│   ├── api/
│   │   ├── mod.rs
│   │   ├── polymarket.rs       # Polymarket REST client (orders)
│   │   └── gamma.rs            # Gamma API (market discovery)
│   │
│   ├── state/
│   │   ├── mod.rs              # AppState struct
│   │   ├── market.rs           # Market info (token IDs, end time)
│   │   ├── book.rs             # Best bid/ask for YES/NO
│   │   ├── position.rs         # Quantities and cost basis
│   │   └── orders.rs           # Order tracker (our open orders)
│   │
│   ├── strategy/
│   │   ├── mod.rs              # Strategy trait
│   │   ├── gaba.rs             # Gaba-style strategy (simple, effective)
│   │   ├── pricing.rs          # Quote calculation
│   │   └── risk.rs             # Limits, circuit breakers, profit lock
│   │
│   └── executor/
│       ├── mod.rs
│       ├── actions.rs          # Action enum (Place, Cancel, Take)
│       └── runner.rs           # Execute actions via API
```

---

## Events

```rust
enum Event {
    // Price feeds
    BtcPrice { price: f64, timestamp_ms: u64 },
    BookUpdate {
        yes_bid: Option<u16>, yes_ask: Option<u16>,
        no_bid: Option<u16>, no_ask: Option<u16>,
        timestamp_ms: u64
    },

    // Order events
    OrderFill { order_id: String, asset_id: String, price: u16, size: f64, is_maker: bool },
    OrderCancelled { order_id: String },

    // System events
    Tick,                                    // Every 1s for housekeeping
    FeedDisconnected { feed: FeedType },
    FeedReconnected { feed: FeedType },
    MarketSwitch { market_id: String },      // Time to switch markets
    Shutdown,
}
```

---

## Actions

```rust
enum Action {
    Place { side: Side, price: u16, size: f64 },
    Cancel { order_id: String },
    CancelAll,
    Take { side: Side, size: f64, max_price: u16 },  // Cross spread
}
```

---

## Strategy: Gaba-Style

### Pricing (Simple)
```rust
fn max_bid(side: Side, book: &Book) -> u16 {
    let opposite_ask = match side {
        Side::Yes => book.no_ask,
        Side::No => book.yes_ask,
    };
    1000 - opposite_ask - MARGIN_TICKS  // MARGIN_TICKS = 5 (0.5c)
}
```

### Sizing (Time-Based)
```rust
fn order_size(seconds_remaining: u64, market_duration: u64) -> f64 {
    let ratio = seconds_remaining as f64 / market_duration as f64;
    if ratio > 0.6 { 12.0 }      // >60% time left
    else if ratio > 0.2 { 10.0 } // 20-60% time left
    else { 6.0 }                 // <20% time left
}
```

### Ladder (Narrow, Expanding)
```rust
const INITIAL_RUNGS: usize = 3;
const RUNG_SPACING: u16 = 10;  // 1c

// Start with 3 rungs just below current ask
// Add new rungs only when market moves to new price levels
```

### Rebalancing (Active Taker)
```rust
fn should_rebalance(position: &Position) -> Option<Action> {
    let imbalance = (position.yes_qty - position.no_qty).abs();
    if imbalance < REBALANCE_THRESHOLD { return None; }  // 30 shares

    let light_side = if position.yes_qty < position.no_qty { Side::Yes } else { Side::No };
    let take_size = (imbalance / 3).min(12.0);  // Partial rebalance

    Some(Action::Take {
        side: light_side,
        size: take_size,
        max_price: 1000 - REBALANCE_MAX_LOSS,  // Accept up to 2c loss
    })
}
```

---

## Polymarket API Reference

### WebSocket Endpoints
- **Market data:** `wss://ws-subscriptions-clob.polymarket.com/ws/market`
- **User channel:** `wss://ws-subscriptions-clob.polymarket.com/ws/user`

### Market WebSocket
```json
// Subscribe
{"type": "subscribe", "channel": "book", "market": "<condition_id>",
 "custom_feature_enabled": true}

// Response: best_bid_ask events (not full depth)
{"event_type": "best_bid_ask", "asset_id": "...",
 "bid": {"price": "0.50", "size": "100"},
 "ask": {"price": "0.52", "size": "50"}}
```

### User WebSocket (Fills)
```json
// Auth required: API key, secret, passphrase
// Receives MATCHED events for fills
{"event_type": "MATCHED", "order_id": "...", "asset_id": "...",
 "price": "0.50", "size": "10", "side": "BUY", "owner": "..."}
```

### REST API
- Base: `https://clob.polymarket.com`
- Auth: L1 signature (wallet) or L2 API keys

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/orders` | POST | Place order(s) |
| `/orders` | DELETE | Cancel all orders |
| `/order/{id}` | DELETE | Cancel specific order |
| `/positions` | GET | Get positions |

### API Limits
- Min order size: 5 shares
- Batch size: 15 orders per call
- Price precision: 0.001 (10 ticks = 1c)
- Size precision: 0.01 shares

---

## Gamma API (Market Discovery)

- Base: `https://gamma-api.polymarket.com`
- Used to find current BTC market by slug pattern

```
GET /markets?slug=btc-updown-15m-{epoch}
GET /markets?slug=btc-updown-5m-{epoch}
```

Returns: `conditionId`, `clobTokenIds` (YES/NO token IDs), `endDate`

---

## Binance WebSocket

- Endpoint: `wss://stream.binance.com:9443/ws/btcusdt@trade`
- Receives: `{"p": "97000.50", "q": "0.1", "T": 1737...}` (price, qty, timestamp)

---

## Configuration

```toml
[general]
mode = "paper"  # or "live"
log_level = "info"
market_type = "15m"  # or "5m"

[credentials]
private_key = "${POLYMARKET_PRIVATE_KEY}"
proxy_wallet = "${POLYMARKET_PROXY_WALLET}"

[strategy]
margin_ticks = 5           # 0.5c margin
initial_rungs = 3          # Start with 3 price levels
rung_spacing = 10          # 1c between rungs
rebalance_threshold = 30   # Shares imbalance before taking

[risk]
max_position = 150         # Max shares per side
profit_lock_usd = 10.0     # Stop at guaranteed profit
circuit_breaker_usd = 200  # Emergency stop
```

---

## Implementation Phases

### Phase 1: Infrastructure (No Strategy)

**Goal:** Receive data, place/cancel orders. Strategy is a stub that logs events.

- [ ] **1.1 Project Setup**
  - [ ] Cargo workspace
  - [ ] Config parsing (TOML + env vars)
  - [ ] Graceful shutdown (SIGTERM/SIGINT)
  - [ ] Structured logging (tracing)

- [ ] **1.2 Binance Feed**
  - [ ] Connect to `wss://stream.binance.com:9443/ws/btcusdt@trade`
  - [ ] Parse trade messages
  - [ ] Auto-reconnect with backoff
  - [ ] Emit `Event::BtcPrice`

- [ ] **1.3 Polymarket Market Feed**
  - [ ] Connect to market WebSocket
  - [ ] Subscribe with `custom_feature_enabled: true`
  - [ ] Parse `best_bid_ask` events
  - [ ] Auto-reconnect with backoff
  - [ ] Emit `Event::BookUpdate`

- [ ] **1.4 Polymarket User Feed**
  - [ ] Connect to user WebSocket
  - [ ] Authenticate with API keys
  - [ ] Parse `MATCHED` fill events
  - [ ] Emit `Event::OrderFill`

- [ ] **1.5 Polymarket REST API**
  - [ ] Order placement (single + batch)
  - [ ] Order cancellation (single + all)
  - [ ] Position query
  - [ ] Request signing (L2 API keys)

- [ ] **1.6 Gamma API**
  - [ ] Market discovery by slug
  - [ ] Parse token IDs and end time

- [ ] **1.7 State Management**
  - [ ] `AppState` struct
  - [ ] `Book` (best bid/ask)
  - [ ] `Position` (quantities, costs)
  - [ ] `OrderTracker` (open orders)

- [ ] **1.8 Event Loop**
  - [ ] Channel setup (mpsc bounded)
  - [ ] Event routing
  - [ ] Stub strategy (logs only)
  - [ ] Timer task (1s ticks)

- [ ] **1.9 Market Switching**
  - [ ] Detect market end (from Gamma)
  - [ ] Switch subscriptions 5s before next market
  - [ ] Cancel all orders on switch

### Phase 2: Strategy Implementation

**Goal:** Implement Gaba-style strategy with paper trading.

- [ ] **2.1 Pricing**
  - [ ] `max_bid = 1000 - ask_opposite - margin`
  - [ ] Time-based sizing
  - [ ] Ladder construction (3 rungs, 1c spacing)

- [ ] **2.2 Order Management**
  - [ ] Diff engine (compare target vs actual)
  - [ ] Place missing orders
  - [ ] Cancel stale orders
  - [ ] Refill after fill (2-24s delay)

- [ ] **2.3 Rebalancing**
  - [ ] Imbalance detection
  - [ ] Taker order generation
  - [ ] Partial rebalance (1/3 of imbalance)

- [ ] **2.4 Risk Management**
  - [ ] Position limits
  - [ ] Profit lock
  - [ ] Circuit breaker
  - [ ] Volatility detection (optional)

- [ ] **2.5 Paper Trading Mode**
  - [ ] Simulate fills when price crosses
  - [ ] Track simulated P&L
  - [ ] Log "would place" messages

### Phase 3: Live Testing

- [ ] Deploy to VPS (QuantVPS)
- [ ] systemd service setup
- [ ] Small size testing (SIZE=5, MAX_POS=20)
- [ ] Monitor and tune

---

## Dependencies

```toml
[package]
name = "polybot-rs"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = { version = "0.24", features = ["native-tls"] }
futures-util = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", features = ["json"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
toml = "0.8"
thiserror = "2"
anyhow = "1"
chrono = "0.4"

# Polymarket signing
ethers = { version = "2", features = ["legacy"] }
hmac = "0.12"
sha2 = "0.10"
base64 = "0.22"
```

---

## Quick Commands

```bash
# Build
cargo build --release

# Run (paper mode)
POLYMARKET_PRIVATE_KEY=... cargo run

# Run with debug logging
RUST_LOG=debug cargo run

# Test specific module
cargo test feeds::binance
```

---

## Reference Links

- Python version: `../polybot/`
- Polymarket CLOB docs: https://docs.polymarket.com
- py-clob-client source: https://github.com/Polymarket/py-clob-client
