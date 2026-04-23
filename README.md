# Trading Bot

High-performance Rust grid trading bot for Bybit spot markets. Designed for maximum uptime and profit extraction from ranging markets.

## Features

- **Grid Trading Strategy** — automated buy-low-sell-high across configurable price bands
- **SQLite Persistence** — order journal survives restarts; state auto-recovers
- **Real P&L Tracking** — daily/weekly/total realized PnL fed into risk engine
- **Risk Engine** — exposure limits, daily/weekly loss caps, max drawdown, kill switch
- **Dry-Run Mode** — simulate orders without touching the exchange
- **Retry + Circuit Breaker** — exponential backoff on API failures, auto-pause on repeated errors
- **Order Deduplication** — `orderLinkId` prevents duplicate placements
- **Clock Sync Check** — warns if local clock drifts from exchange server time
- **Dynamic Grid Rebalancing** — auto-shifts grid when price nears the edge
- **Alerts** — Slack/Discord/webhook notifications for fills, risk events, errors, P&L summaries
- **Graceful Shutdown** — waits for in-flight orders, cancels remaining, saves state

## Quick Start

```bash
# 1. Clone and build
git clone <repo-url> && cd trading-bot
cargo build --release

# 2. Configure
cp config.example.toml config.toml
# Edit config.toml with your Bybit API credentials

# 3. Run (testnet first!)
cargo run --release -- config.toml

# 4. Dry-run (no real orders)
cargo run --release -- config.toml --dry-run
```

## Configuration

See [`config.example.toml`](config.example.toml) for all options. Key sections:

| Section | Purpose |
|---------|---------|
| `[bot]` | dry_run, db_path, retry/circuit-breaker settings |
| `[bybit]` | API credentials, testnet toggle |
| `[grid]` | symbol, price range, levels, qty, rebalancing |
| `[risk]` | exposure caps, loss limits, drawdown |
| `[alerts]` | webhook URL, notification filters |

## Architecture

```
main.rs          Entry point, CLI args
bot.rs           Orchestration: startup -> poll loop -> shutdown
grid.rs          Pure grid math (no I/O)
risk.rs          Pre-trade risk checks (no I/O)
persistence.rs   SQLite order journal + P&L storage
retry.rs         Exponential backoff + circuit breaker
alerts.rs        Webhook notifications
config.rs        TOML config loader
market_hours.rs  Session windows (crypto/NSE/MCX/US)
venues/bybit/    REST + WebSocket + HMAC signing
```

## How Grid Trading Works

1. Bot places **buy orders below** and **sell orders above** the current price
2. When a **buy fills**, a sell is placed one level up (locking in the spread as profit)
3. When a **sell fills**, a buy is placed one level down (ready to catch the next dip)
4. Each complete buy-sell cycle earns approximately `grid_spacing * qty`

**Profit is maximized in ranging/sideways markets.** In trending markets, enable `auto_rebalance` to shift the grid.

## Risk Controls

| Control | What it does |
|---------|-------------|
| Kill switch | Ctrl+C trips atomic flag, stops all trading |
| Daily loss limit | Pauses new orders if daily P&L exceeds threshold |
| Weekly loss limit | Same, over rolling 7 days |
| Max drawdown | Pauses if equity drops too far from peak |
| Per-symbol exposure | Caps notional per trading pair |
| Circuit breaker | Pauses after N consecutive API failures |

## Database

SQLite database (`trading-bot.db`) stores:
- **orders** — full order lifecycle (placed, filled, cancelled)
- **fills** — every fill with realized P&L
- **bot_state** — key-value for recovery metadata

Query your P&L:
```bash
sqlite3 trading-bot.db "SELECT date(filled_at) as day, SUM(realised_pnl) as pnl, COUNT(*) as fills FROM fills GROUP BY day ORDER BY day;"
```

## Environment Variables

- `RUST_LOG=debug` — verbose logging
- `RUST_LOG=trading_bot=debug,reqwest=warn` — debug bot, quiet HTTP

## Testing

```bash
cargo test
```

## License

MIT
