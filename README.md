# Zooni

[![CI](https://github.com/syedhamidali/zooni/actions/workflows/ci.yml/badge.svg)](https://github.com/syedhamidali/zooni/actions/workflows/ci.yml)
[![Coverage](https://codecov.io/gh/syedhamidali/zooni/branch/main/graph/badge.svg)](https://codecov.io/gh/syedhamidali/zooni)
[![Security Audit](https://github.com/syedhamidali/zooni/actions/workflows/audit.yml/badge.svg)](https://github.com/syedhamidali/zooni/actions/workflows/audit.yml)
[![Dependency Check](https://github.com/syedhamidali/zooni/actions/workflows/deny.yml/badge.svg)](https://github.com/syedhamidali/zooni/actions/workflows/deny.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

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

- [Rust](https://rustup.rs/) 1.85+ (edition 2024)
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
./target/release/zooni config.toml

# Dry-run mode — simulate without hitting the exchange
./target/release/zooni config.toml --dry-run

# TUI dashboard — live terminal view
./target/release/zooni config.toml --tui

# Auto-pilot — fully autonomous
./target/release/zooni config.toml --autopilot

# Check version
./target/release/zooni --version
```

## Binaries

| Binary | Description |
|--------|-------------|
| `zooni` | Main bot — grid trading, autopilot, dashboard, Telegram |
| `zooni-backtest` | Backtest grid strategies against CSV or synthetic data |
| `zooni-scan` | One-shot market scanner — rank symbols by grid suitability |

### Backtester

```bash
cargo run --release --bin zooni-backtest -- config.toml --data prices.csv
cargo run --release --bin zooni-backtest -- config.toml --optimize --data prices.csv
```

### Scanner

```bash
cargo run --release --bin zooni-scan -- --config config.toml --top 10
```

## How Grid Trading Works

1. Bot places **buy orders below** and **sell orders above** the current price
2. When a **buy fills**, a sell is placed one level up (locking in the spread as profit)
3. When a **sell fills**, a buy is placed one level down (ready to catch the next dip)
4. Each complete buy-sell cycle earns approximately `grid_spacing * qty`

**Profit is maximized in ranging/sideways markets.** In trending markets, enable `auto_rebalance` to shift the grid, or use auto-pilot mode which detects regimes and switches symbols automatically.

## Configuration

See [`config.example.toml`](config.example.toml) for the full reference with inline comments.

| Section | Purpose |
|---------|---------|
| `[bot]` | Dry-run, database path, retry/circuit-breaker settings |
| `[bybit]` | Exchange API credentials and endpoint settings |
| `[grid]` | Grid strategy parameters (symbol, range, levels, qty) |
| `[risk]` | Exposure limits, loss caps, drawdown thresholds |
| `[alerts]` | Webhook URL and notification filters |
| `[autopilot]` | Autonomous mode settings |
| `[autopilot.scanner]` | Market scanner tuning |

### Telegram

```bash
export TELEGRAM_BOT_TOKEN="123456:ABC-DEF..."
export TELEGRAM_CHAT_ID="987654321"
```

Commands: `/status`, `/pnl`, `/grid`, `/pause`, `/resume`, `/fills`

## Dashboard

The web dashboard runs at `http://localhost:9090`:

| Endpoint | Description |
|----------|-------------|
| `/` | Interactive HTML dashboard |
| `/health` | Health check |
| `/status` | JSON status snapshot |
| `/metrics` | Prometheus metrics |
| `/api/live` | Full live data (JSON) |

## Deployment

### Docker

```bash
docker build -t zooni .
docker run -p 9090:9090 -v $(pwd)/config.toml:/data/config.toml zooni
```

### Systemd (VPS)

```bash
sudo useradd -r -s /bin/false zooni
sudo mkdir -p /opt/zooni
sudo cp target/release/zooni /opt/zooni/
sudo cp config.toml /opt/zooni/
sudo cp zooni.service /etc/systemd/system/
sudo systemctl enable --now zooni
sudo journalctl -u zooni -f
```

## Database

SQLite database (`zooni.db`) stores orders, fills with realized P&L, and recovery state.

```bash
sqlite3 zooni.db "SELECT date(filled_at) as day, SUM(realised_pnl) as pnl, COUNT(*) as fills FROM fills GROUP BY day ORDER BY day;"
```

## Testing

```bash
cargo test --all           # 80+ unit & integration tests
cargo clippy --all-targets # lint
cargo fmt --all -- --check # format check
cargo bench                # criterion benchmarks
```

## CI/CD

GitHub Actions on every push and PR to `main`:

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| **CI** | push, PR | Auto-format, clippy, tests, release build + artifact upload |
| **Coverage** | push, PR | cargo-tarpaulin → Codecov |
| **Security Audit** | push, PR, weekly | cargo-audit via RustSec advisory DB |
| **Dependency Check** | push, PR | cargo-deny (licenses, bans, sources) |
| **Auto-Release** | push to main | Date-tagged GitHub release with binaries |
| **Deploy Docs** | push (docs changed) | mdBook → Cloudflare Pages (password-gated) |

## Versioning

Date-based: `YYYY.M.D` with git hash. Auto-embedded at compile time.

```
$ zooni --version
zooni 2026.4.23 (29db06d)
```

## Supported Venues

| Venue | Status | Notes |
|-------|--------|-------|
| Bybit | Active | Spot V5 API, testnet supported |
| Hyperliquid | Planned | Skeleton implemented |
| Zerodha | Planned | Indian equities via Kite Connect v3 |
| Interactive Brokers | Planned | US equities via TWS/IB Gateway |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

## Security

See [SECURITY.md](SECURITY.md) for reporting vulnerabilities.

## License

[MIT](LICENSE)
