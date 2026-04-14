# polybot-rs

Market-making bot for Polymarket BTC binary options (5-min markets). Quotes both sides of the book and reacts to a live Binance BTC feed.

## Strategy

Quote both sides (YES and NO) of a binary market so that `yes_bid + no_bid < $1`. Every matched pair is a risk-free payout of $1 at resolution, so the edge per pair is `1 - (yes_bid + no_bid)`.

Quotes come from an Avellaneda-Stoikov pricer operating in logit space:

- **Reservation price** shifts against inventory: `r = x - q * gamma * var * T` (x = logit of mid, q = net inventory, T = time to close).
- **Spread** widens with variance and tightens with order flow: `spread = gamma * var * T + (2/k) * ln(1 + gamma/k)`.
- Bid/ask are converted back to probabilities; `yes_bid = p_bid`, `no_bid = 1 - p_ask`.

`var` is a rolling logit-return variance, `k` is an EWMA of trade intensity. Order size tapers as the market approaches expiry. A BTC guard cancels all resting orders on sharp BTC moves (default: 0.3% in 2s) to avoid adverse selection.

## Architecture

Single process, async Tokio. Four WebSocket feeds fan into one bounded mpsc channel; a 50ms tick loop owns all state and issues order actions.

```
  Binance WS ─┐
  Polymarket WS ─┼──► mpsc ──► tick loop (50ms) ──► executor ──► CLOB REST
  User WS ────┤                  │
  Gamma API ──┘                  ├─ book / position / order tracker
                                 ├─ variance / flow / BTC guard
                                 └─ A-S pricer ─► reconcile ─► Place/Cancel
```

- `feeds/` — Binance trades, Polymarket order book, user fill stream
- `api/gamma.rs` — market discovery (next 5-min BTC market)
- `state/` — book, position, resting order tracker
- `strategy/` — A-S pricer, sizing, variance, flow, BTC guard, actions
- `executor.rs` — signs and submits CLOB orders, updates tracker on acks/fills
- `logging.rs` — per-session CSV with ticks, fills, cancels, window stats
- `main.rs` — event loop, constants, reconcile logic

## Requirements

- Rust (stable, edition 2021)
- A Polymarket account with a proxy wallet and USDC on Polygon
- Polygon RPC endpoint (defaults to `https://polygon-rpc.com`)

## Setup

Create a `.env` file in the project root:

```
POLY_PRIVATE_KEY=0x...
POLY_PROXY_WALLET=0x...
POLYGON_RPC_URL=https://polygon-rpc.com   # optional
```

- `POLY_PRIVATE_KEY` — private key of the EOA that signs orders
- `POLY_PROXY_WALLET` — Polymarket proxy wallet address (the funder)

On first run, the CLOB API key is derived automatically from the private key.

## Build

```bash
cargo build --release
```

## Run

```bash
# Live trading
cargo run --release

# Dry run (no orders placed)
cargo run --release -- --log-only

# Trade N markets then exit
cargo run --release -- --markets 3

# Dry run limited to 1 market
cargo run --release -- --log-only --markets 1
```

Flags:

| Flag | Description |
|------|-------------|
| `--log-only`, `--dry-run` | Log ticks and quotes without placing orders |
| `--markets N` | Exit after trading `N` markets |

## Logs

Each session writes a CSV to `logs/polybot_<timestamp>.csv` with tick state, fills, cancels, halts, and per-window/session summaries.

## Tuning

Strategy constants (tick rate, A-S gamma, variance window, BTC guard thresholds, order size, halt/warmup buffers) live at the top of `src/main.rs`.

## Extra binaries

```bash
cargo run --release --bin redeem               # redeem winning positions
cargo run --release --bin test_executor        # exercise order placement
cargo run --release --bin test_order           # place a single test order
cargo run --release --bin test_user_ws         # stream user fill events
cargo run --release --bin test_pricing         # A-S pricer sanity check
cargo run --release --bin test_variance        # variance estimator
cargo run --release --bin test_flow            # flow estimator
cargo run --release --bin test_ws_speed        # Binance WS latency probe
```

## Tests

```bash
cargo test
cargo check
```
