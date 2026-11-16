# polybot-rs

Market-making bot for Polymarket BTC binary options (5-min "up/down" markets). Quotes both sides of the book and uses a live Binance BTC feed to pull quotes on sharp moves.

## Strategy

Quote both sides (YES and NO) of a binary market so that `yes_bid + no_bid < $1`. One YES plus one NO always pays exactly $1, so every matched pair locks in `1 - (yes_bid + no_bid)`. At the end of each window, matched pairs are merged back into USDC on-chain; unmatched shares are held to resolution. The main risk is legging: only one side filling, leaving a directional position.

Quotes come from an Avellaneda-Stoikov-style pricer operating in logit space:

- **Reservation price** shifts against inventory: `r = x - q * gamma * var * T` (x = logit of mid, q = net inventory in shares, T = seconds to close).
- **Spread** widens with variance and tightens with trade flow: `spread = gamma * var * T + (2/k) * ln(1 + gamma/k)`. This modifies the textbook flow term (`2/gamma`) so trade intensity drives the spread directly.
- Bid/ask are converted back to probabilities: `yes_bid = p_bid`, `no_bid = 1 - p_ask`.

Inputs:

- `mid` — YES mid from the Polymarket book.
- `var` — variance of logit mid changes per second over the last 120 book updates (floor 0.001).
- `k` — trades per second, counted over a rolling 30s window (floor 0.1).

Each side holds a single resting 5-share order. Every 50ms the target price is recomputed; if it differs from the resting price, the order is cancelled and replaced.

## Risk controls

| Control | Behavior |
|---------|----------|
| Warmup | No quoting for the first 15s of a market |
| Expiry halt | Cancel all and stop quoting at T-15s; extra spread floor below T-30s |
| Mid range | No quoting when the YES mid is outside 7%–93% |
| Stale book | Cancel all if no book update for 5s |
| BTC guard | Cancel all on a 0.3% BTC move (up or down) within 2s, then pause 5s |
| Stay maker | Bids are clamped at least 1¢ below the current ask |
| Pair-cost cap | A side's bid is capped so `bid + avg cost of the other side ≤ 99¢` |
| Fill cooldown | A side is not re-quoted for 1s after a fill |

## Architecture

Single process, async Tokio. Three WebSocket feeds parse in their own tasks and send typed events over one bounded mpsc channel. A single event loop owns all state (no locks): it applies events as they arrive and runs the strategy on a 50ms tick.

```
  Binance WS ────┐
  Polymarket WS ─┼──► mpsc ──► event loop ──► executor ──► CLOB REST
  User WS ───────┘                │
                                  ├─ book / position / order tracker
                                  ├─ variance / flow / BTC guard
                                  └─ A-S pricer ─► reconcile ─► Place/Cancel

  Gamma REST (market discovery) and Polygon (merge) are called directly
  from the event loop at market switch.
```

- `feeds/` — Binance trades, Polymarket best bid/ask and trades, user fill stream
- `api/gamma.rs` — market discovery by slug (`btc-updown-5m-<epoch>`)
- `state/` — book, position, resting order tracker
- `strategy/` — A-S pricer, variance, flow, BTC guard, actions
- `executor.rs` — signs and submits CLOB orders and cancels, updates the order tracker on acks
- `logging.rs` — per-session CSV plus console output
- `main.rs` — event loop, constants, reconcile logic, fill handling, market switching and merge

## Requirements

- Rust (stable, edition 2021)
- A Polymarket account with a proxy wallet and USDC on Polygon
- Polygon RPC endpoint (defaults to `https://polygon-rpc.com`)
- A network location that can reach Binance (`stream.binance.com` blocks US IPs)

## Setup

Create a `.env` file in the project root:

```
POLY_PRIVATE_KEY=0x...
POLY_PROXY_WALLET=0x...
POLYGON_RPC_URL=https://polygon-rpc.com   # optional
```

- `POLY_PRIVATE_KEY` — private key of the EOA that signs orders
- `POLY_PROXY_WALLET` — Polymarket proxy wallet address (the funder)

On startup, the CLOB API key is created or derived from the private key. Credentials are required even for dry runs.

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
| `--log-only`, `--dry-run` | Compute and log quotes without placing orders or merging |
| `--markets N` | Exit after trading `N` markets |

Set `RUST_LOG` (default `info`) to control executor log verbosity.

## Logs

Each session writes a CSV to `logs/polybot_<timestamp>.csv` with rows for ticks (quotes and state), halts, replace-cancels, fills, BTC shocks, and window start/end (position, min P&L, parameters). A session summary is printed to the console on exit.

## Tuning

Strategy constants (tick rate, A-S gamma, variance window, flow window, BTC guard thresholds, order size, halt/warmup buffers, pair-cost cap) live at the top of `src/main.rs`.

## Extra binaries

```bash
cargo run --release --bin redeem               # redeem resolved positions and merge pairs
cargo run --release --bin test_pricing         # live A-S quotes vs. the book (no orders)
cargo run --release --bin test_variance        # live variance estimator (no orders)
cargo run --release --bin test_flow            # live trade-rate estimator (no orders)
cargo run --release --bin test_user_ws         # stream user fill events
cargo run --release --bin test_ws_speed        # Binance JSON parse time (simd-json)

# These place REAL orders (5 shares @ 1¢ on the current 15m market, then cancel):
cargo run --release --bin test_order           # place + cancel via polyfill-rs, timed
cargo run --release --bin test_order_official  # same via the official Polymarket SDK
cargo run --release --bin test_executor        # place YES/NO via the executor, then cancel-all
```

## Tests

```bash
cargo test
cargo check
```
