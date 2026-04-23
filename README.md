# Zooni

Autonomous algorithmic trading bot for cryptocurrency markets. Zooni runs grid trading strategies on Bybit, with built-in market intelligence to pick symbols, size positions, and switch between opportunities automatically.

## Features

- **Grid Trading** — layered limit orders across a price range, capturing profit from volatility
- **Auto-Pilot Mode** — scans markets, picks the best symbol, deploys grids, and switches when better opportunities appear
- **Market Intelligence** — RSI, ADX, ATR, Bollinger Bands, SMA, EMA, VWAP for regime detection
- **Risk Engine** — per-symbol, per-strategy, and portfolio-wide exposure limits with daily/weekly loss caps and max drawdown
- **Smart Sizing** — Kelly criterion blended with regime-adjusted scaling
- **Web Dashboard** — real-time charts, grid visualization, and P&L at `http://localhost:9090`
- **TUI Dashboard** — terminal-based live view with `--tui`
- **Telegram Bot** — remote `/status`, `/pnl`, `/pause`, `/resume` commands
- **Webhook Alerts** — Slack, Discord, or any HTTP POST endpoint
- **Backtesting** — run strategies against historical or synthetic data with optimizer sweep
- **Circuit Breaker** — pauses API calls after consecutive failures, auto-resets after cooldown
- **Rate Limiting** — token bucket to stay within exchange API limits

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) (edition 2024)
- A Bybit account (testnet recommended for initial setup)

### Build

```bash
cargo build --release
```

### Configure

```bash
cp config.example.toml config.toml
# Edit config.toml with your API keys and parameters
```

> **Never commit `config.toml`** — it contains secrets. It is already in `.gitignore`.

### Run

```bash
# Standard mode — trade a single configured symbol
./target/release/trading-bot config.toml

# Dry-run mode — simulate without hitting the exchange
./target/release/trading-bot config.toml --dry-run

# TUI dashboard — live terminal view
./target/release/trading-bot config.toml --tui

# Auto-pilot — fully autonomous (set autopilot.enabled = true in config)
./target/release/trading-bot config.toml --autopilot

# Check version
./target/release/trading-bot --version
```

## Binaries

| Binary | Description |
|--------|-------------|
| `trading-bot` | Main bot — grid trading, autopilot, dashboard, Telegram |
| `backtest` | Backtest grid strategies against CSV or synthetic data |
| `scan` | One-shot market scanner — rank symbols by grid suitability |

### Backtester

```bash
# Against historical data
cargo run --release --bin backtest -- config.toml --data prices.csv

# Against synthetic data (5000 bars)
cargo run --release --bin backtest -- config.toml --synthetic 5000

# Optimizer sweep
cargo run --release --bin backtest -- config.toml --optimize --data prices.csv
```

### Scanner

```bash
# Scan top USDT pairs
cargo run --release --bin scan -- --config config.toml --top 10

# Scan with custom quote currency
cargo run --release --bin scan -- --config config.toml --quote USDT --min-turnover 5000000
```

## How Grid Trading Works

1. Bot places **buy orders below** and **sell orders above** the current price
2. When a **buy fills**, a sell is placed one level up (locking in the spread as profit)
3. When a **sell fills**, a buy is placed one level down (ready to catch the next dip)
4. Each complete buy-sell cycle earns approximately `grid_spacing * qty`

**Profit is maximized in ranging/sideways markets.** In trending markets, enable `auto_rebalance` to shift the grid, or use auto-pilot mode which detects regimes and switches symbols automatically.

## Configuration

See [`config.example.toml`](config.example.toml) for the full reference. Key sections:

### `[bybit]` — Exchange Credentials

| Field | Default | Description |
|-------|---------|-------------|
| `api_key` | — | Bybit API key |
| `api_secret` | — | Bybit API secret |
| `testnet` | `true` | Use testnet endpoints |
| `account_type` | `"UNIFIED"` | Wallet type for balance lookups |

### `[grid]` — Grid Strategy

| Field | Default | Description |
|-------|---------|-------------|
| `symbol` | — | Trading pair (e.g., `"BTCUSDT"`) |
| `lower_price` | — | Grid lower bound |
| `upper_price` | — | Grid upper bound |
| `levels` | — | Number of grid lines (min 2) |
| `qty_per_level_base` | — | Base asset quantity per order |
| `poll_secs` | `3` | Fill-detection polling interval |
| `auto_rebalance` | `false` | Shift grid when price exits range |
| `trailing_profit_threshold` | `0.0` | Lock in gains beyond this USDT threshold |

### `[risk]` — Risk Management

| Field | Default | Description |
|-------|---------|-------------|
| `starting_equity_quote` | — | Starting balance in USDT |
| `max_total_exposure` | `0.80` | Max portfolio exposure (80%) |
| `max_per_symbol` | `0.20` | Max exposure per symbol |
| `daily_loss_limit` | `0.05` | Daily loss limit (5%) — pauses trading |
| `weekly_loss_limit` | `0.10` | Weekly loss limit (10%) |
| `max_drawdown` | `0.15` | Max drawdown from equity peak (15%) |

### `[autopilot]` — Autonomous Mode

| Field | Default | Description |
|-------|---------|-------------|
| `enabled` | `false` | Enable fully autonomous trading |
| `scan_interval_secs` | `1800` | Re-scan markets every 30 minutes |
| `switch_threshold` | `15.0` | Score improvement needed to switch symbols |
| `max_grids` | `1` | Number of simultaneous grids |

### `[alerts]` — Notifications

| Field | Default | Description |
|-------|---------|-------------|
| `webhook_url` | — | Slack/Discord/HTTP webhook URL |
| `on_fill` | `false` | Alert on every fill |
| `on_risk_breach` | `true` | Alert on risk limit breach |
| `on_error` | `true` | Alert on API errors |
| `pnl_summary_interval_secs` | `0` | Periodic P&L summary (0 = disabled) |

### `[bot]` — General Settings

| Field | Default | Description |
|-------|---------|-------------|
| `dry_run` | `false` | Simulate orders without exchange calls |
| `db_path` | `"trading-bot.db"` | SQLite database path |
| `max_retries` | `3` | API call retry count |
| `circuit_breaker_threshold` | `5` | Consecutive failures before pausing |
| `circuit_breaker_cooldown_secs` | `60` | Cooldown before retry after circuit break |
| `shutdown_wait_secs` | `10` | Wait for in-flight orders at shutdown |

### Telegram

Set these environment variables to enable the Telegram bot:

```bash
export TELEGRAM_BOT_TOKEN="123456:ABC-DEF..."
export TELEGRAM_CHAT_ID="987654321"
```

Commands: `/status`, `/pnl`, `/grid`, `/pause`, `/resume`, `/fills`

## Dashboard

The web dashboard runs at `http://localhost:9090` when the bot starts:

- Live price and equity charts
- Grid level visualization
- Recent fills table
- P&L stats (daily/total)

| Endpoint | Description |
|----------|-------------|
| `/` | Interactive HTML dashboard |
| `/health` | Health check |
| `/status` | JSON status snapshot |
| `/metrics` | Prometheus metrics |
| `/api/live` | Full live data (JSON) |

## Technical Indicators

Used by the scanner and regime detector to evaluate symbols:

| Indicator | Purpose |
|-----------|---------|
| SMA | Simple moving average — trend direction |
| EMA | Exponential moving average — responsive trend |
| RSI | Relative Strength Index — overbought/oversold |
| ADX | Average Directional Index — trend strength |
| ATR | Average True Range — volatility measure |
| Bollinger Bands | Volatility envelope — range detection |
| VWAP | Volume-weighted price — fair value reference |

### Market Regimes

The regime detector classifies the market into four states:

| Regime | Description | Grid Suitability |
|--------|-------------|-----------------|
| Ranging | Low ADX, neutral RSI, tight bands | Best |
| Trending Up | High ADX, high RSI | Poor |
| Trending Down | High ADX, low RSI | Poor |
| Volatile | Wide Bollinger bands | Risky |

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

## Architecture

```
src/
  main.rs             Entry point, CLI args, --version
  bot.rs              Orchestration: startup -> poll loop -> shutdown
  grid.rs             Pure grid math (no I/O)
  autopilot.rs        Autonomous scan -> deploy -> switch cycle
  config.rs           TOML config loader + validation
  risk.rs             Pre-trade risk checks
  persistence.rs      SQLite order journal + P&L storage
  indicators.rs       Technical indicators (RSI, ADX, ATR, BB, etc.)
  regime.rs           Market regime detection + classification
  scanner.rs          Market scanner — rank symbols by suitability
  sizing.rs           Kelly criterion + regime-adjusted position sizing
  metrics.rs          Web dashboard + Prometheus metrics
  telegram.rs         Telegram bot for remote control
  tui.rs              Terminal UI dashboard (ratatui)
  alerts.rs           Webhook notifications
  retry.rs            Exponential backoff + circuit breaker
  rate_limit.rs       Token bucket rate limiter
  market_hours.rs     Session windows (crypto/NSE/MCX/US)
  strategy/
    mod.rs            Strategy trait + action enum
    dca.rs            Dollar-cost averaging strategy
  venues/
    bybit/            REST + WebSocket + HMAC signing
    hyperliquid/      (planned)
    zerodha/          (planned)
    interactive_brokers/  (planned)
```

## Deployment

### Docker

```bash
docker build -t zooni .
docker run -v $(pwd)/config.toml:/data/config.toml zooni
```

### Systemd (VPS)

```bash
# Create service user
sudo useradd -r -s /bin/false trading

# Install binary
sudo mkdir -p /opt/trading-bot
sudo cp target/release/trading-bot /opt/trading-bot/
sudo cp config.toml /opt/trading-bot/

# Install and start service
sudo cp trading-bot.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now trading-bot

# View logs
sudo journalctl -u trading-bot -f
```

## Versioning

Zooni uses date-based versioning: `YYYY.M.D` (e.g., `2026.4.23`). The version and git hash are embedded at compile time via `build.rs`.

```
$ zooni --version
zooni 2026.4.23 (29db06d)
```

## Testing

```bash
cargo test --all           # run all 64 tests
cargo clippy --all-targets # lint
cargo fmt --all -- --check # format check
```

## CI/CD

GitHub Actions runs on every push and PR to `main`:

1. **Check** — `cargo fmt --check` + `cargo clippy`
2. **Test** — `cargo test --all`
3. **Build** — release binaries uploaded as artifacts
4. **Release** — auto-creates a GitHub release with date-based tag on push to `main`

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `RUST_LOG` | Log level (e.g., `info`, `debug`, `trading_bot=debug,reqwest=warn`) |
| `TELEGRAM_BOT_TOKEN` | Telegram bot token for remote control |
| `TELEGRAM_CHAT_ID` | Telegram chat ID for notifications |

## Supported Venues

| Venue | Status | Notes |
|-------|--------|-------|
| Bybit | Active | Spot V5 API, testnet supported |
| Hyperliquid | Planned | Skeleton implemented |
| Zerodha | Planned | Indian equities via Kite Connect v3 |
| Interactive Brokers | Planned | US equities via TWS/IB Gateway |

## License

Private — all rights reserved.
