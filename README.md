# Zooni

[![CI](https://github.com/syedhamidali/zooni/actions/workflows/ci.yml/badge.svg)](https://github.com/syedhamidali/zooni/actions/workflows/ci.yml)
[![Coverage](https://codecov.io/gh/syedhamidali/zooni/branch/main/graph/badge.svg)](https://codecov.io/gh/syedhamidali/zooni)
[![Security Audit](https://github.com/syedhamidali/zooni/actions/workflows/audit.yml/badge.svg)](https://github.com/syedhamidali/zooni/actions/workflows/audit.yml)
[![Dependency Check](https://github.com/syedhamidali/zooni/actions/workflows/deny.yml/badge.svg)](https://github.com/syedhamidali/zooni/actions/workflows/deny.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)](https://www.rust-lang.org)

Autonomous algorithmic trading bot for crypto and Indian equity markets. Multi-strategy orchestrator with adaptive capital allocation, running on Bybit (crypto) and Zerodha Kite Connect (NSE/BSE/F&O/MCX).

## Features

### Trading
- **Multi-Strategy Orchestrator** — runs DCA, Momentum, Grid, and Pairs strategies simultaneously with adaptive capital allocation via multi-armed bandit (softmax over EMA-smoothed risk-adjusted returns)
- **Grid Trading** — layered limit orders across a price range, capturing profit from volatility
- **Pairs / Statistical Arbitrage** — market-neutral spread trading on correlated pairs (e.g., HDFC/ICICI, ETH/BTC)
- **Funding Rate Arbitrage** — delta-neutral carry trade: long spot + short perpetual, collect funding (Bybit)
- **Auto-Pilot Mode** — scans markets, picks the best symbol, deploys grids, and switches when better opportunities appear

### Intelligence
- **Market Regime Detection** — RSI, ADX, ATR, Bollinger Bands, SMA, EMA, VWAP classify Ranging/Trending/Volatile
- **Equity Curve Risk Guard** — auto-disables strategies exceeding drawdown threshold, re-enables after cooldown
- **Smart Sizing** — Kelly criterion blended with regime-adjusted scaling
- **Tax-Aware Profitability** — India 30% crypto tax + 1% TDS, equity STCG/LTCG factored into entry decisions
- **Slippage Protection** — reject adverse fills, convert market orders to aggressive limits

### Infrastructure
- **Multi-Venue** — Bybit (crypto spot/perps) and Zerodha Kite Connect (Indian equity/F&O/MCX) via unified venue abstraction
- **Yahoo Finance** — free historical data for backtesting and regime detection (no paid API needed)
- **Risk Engine** — per-symbol, per-strategy, and portfolio-wide exposure limits with daily/weekly loss caps and max drawdown
- **Web Dashboard** — real-time charts, grid visualization, and P&L at `http://localhost:9090`
- **TUI Dashboard** — terminal-based live view with `--tui`
- **Telegram Bot** — remote `/status`, `/pnl`, `/alloc`, `/regime`, `/pause`, `/resume` + proactive alerts (regime changes, strategy disables, P&L thresholds)
- **Webhook Alerts** — Slack, Discord, or any HTTP POST endpoint
- **Backtesting** — grid, orchestrator, walk-forward analysis, optimizer sweep, CSV export
- **Circuit Breaker** — pauses API calls after consecutive failures, auto-resets after cooldown
- **Rate Limiting** — token bucket to stay within exchange API limits

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) 1.85+ (edition 2024)
- A Bybit account (testnet recommended) **or** a Zerodha Kite Connect account

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

### Run (Bybit — crypto)

```bash
# Grid bot — trade a single configured symbol
./target/release/zooni config.toml

# Multi-strategy orchestrator — DCA + Momentum + adaptive allocation
./target/release/zooni config.toml --orchestrator

# Funding rate arbitrage — delta-neutral carry trade
./target/release/zooni config.toml --funding-arb

# Auto-pilot — fully autonomous symbol selection
./target/release/zooni config.toml --autopilot

# Dry-run mode — simulate without hitting the exchange
./target/release/zooni config.toml --dry-run

# TUI dashboard — live terminal view
./target/release/zooni config.toml --tui
```

### Run (Zerodha — Indian equities)

```bash
# Daily login (run each morning — token valid until ~06:00 IST next day)
./target/release/zooni-kite-login config.toml

# Run orchestrator on NSE equities
./target/release/zooni config.toml --venue zerodha --orchestrator

# Dry-run first to verify
./target/release/zooni config.toml --venue zerodha --dry-run
```

## Binaries

| Binary | Description |
|--------|-------------|
| `zooni` | Main bot — grid, orchestrator, autopilot, funding-arb, TUI, Telegram |
| `zooni-backtest` | Backtest strategies against CSV, synthetic, or Yahoo Finance data |
| `zooni-scan` | One-shot market scanner — rank symbols by grid suitability |
| `zooni-kite-login` | Zerodha Kite Connect daily login helper (OAuth token exchange) |

### Backtester

```bash
# From CSV data
zooni-backtest config.toml --data prices.csv

# Synthetic data
zooni-backtest config.toml --synthetic 5000

# Yahoo Finance — Indian equities (free, no API key needed)
zooni-backtest config.toml --yahoo RELIANCE.NS --orchestrator
zooni-backtest config.toml --yahoo HDFCBANK.NS --yahoo-range 1y --orchestrator
zooni-backtest config.toml --yahoo TCS.NS --yahoo-interval 1h --yahoo-range 1mo

# Pairs backtest with real data
zooni-backtest config.toml --orchestrator --pairs-data ETHUSDT,BTCUSDT

# Walk-forward analysis
zooni-backtest config.toml --walk-forward --synthetic 5000

# Optimizer sweep
zooni-backtest config.toml --optimize --data prices.csv

# Export results to CSV
zooni-backtest config.toml --yahoo INFY.NS --orchestrator --export results.csv
```

### Scanner

```bash
zooni-scan --config config.toml --top 10
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
| `[bybit]` | Bybit API credentials and endpoint settings |
| `[zerodha]` | Zerodha Kite Connect credentials, exchange, product |
| `[grid]` | Grid strategy parameters (symbol, range, levels, qty) |
| `[risk]` | Exposure limits, loss caps, drawdown thresholds |
| `[orchestrator]` | Multi-strategy settings, per-strategy config, risk guard |
| `[alerts]` | Webhook URL and notification filters |
| `[telegram]` | Telegram bot token, chat ID, poll interval |
| `[autopilot]` | Autonomous mode settings |
| `[autopilot.scanner]` | Market scanner tuning |
| `[funding_arb]` | Funding rate arbitrage thresholds and limits |
| `[tax]` | Tax-aware profitability (India crypto/equity rates) |
| `[slippage]` | Slippage protection and limit order conversion |

### Telegram

Configure in `config.toml` or via environment variables:

```toml
[telegram]
bot_token  = "123456:ABC-DEF..."
chat_id    = "987654321"
poll_secs  = 5
```

Commands: `/status`, `/pnl`, `/alloc`, `/regime`, `/pause`, `/resume`, `/help`

Proactive alerts are sent automatically for regime changes, strategy disables, significant rebalances, and P&L threshold crossings.

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
cargo test --all           # 300+ unit & integration tests
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
| Bybit | Active | Spot + linear perps, V5 API, testnet supported |
| Zerodha | Active | NSE/BSE/NFO/MCX via Kite Connect v3 (Personal plan — free) |
| Yahoo Finance | Active | Free historical data for backtesting and regime detection |
| Hyperliquid | Planned | Skeleton implemented |
| Interactive Brokers | Planned | US equities via TWS/IB Gateway |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

## Security

See [SECURITY.md](SECURITY.md) for reporting vulnerabilities.

## License

[MIT](LICENSE)
