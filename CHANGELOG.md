# Changelog

All notable changes to Zooni will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Zooni uses date-based versioning: `YYYY.M.D`.

## [2026.4.24]

### Added
- **Multi-venue support** — unified venue abstraction (`Venue` enum) with Bybit and Zerodha backends
- **Zerodha Kite Connect** — full REST client (orders, portfolio, margins, quotes, instruments CSV) and binary WebSocket ticker
- **Kite login helper** (`zooni-kite-login`) — OAuth flow with local HTTP callback for daily access token refresh
- **Yahoo Finance integration** — free historical OHLCV data for backtesting and regime detection (no API key needed)
- **Multi-strategy orchestrator** (`--orchestrator`) — runs DCA, Momentum, Grid, and Pairs strategies simultaneously
- **Adaptive capital allocation** — multi-armed bandit (softmax over EMA-smoothed risk-adjusted returns) shifts capital to winning strategies
- **Pairs / statistical arbitrage strategy** — market-neutral spread trading on correlated pairs
- **Funding rate arbitrage** (`--funding-arb`) — delta-neutral carry trade: long spot + short perpetual
- **Proactive Telegram alerts** — regime changes, strategy disables, significant rebalances, P&L threshold crossings
- **Equity curve risk guard** — auto-disables strategies exceeding drawdown threshold, re-enables after cooldown
- **Walk-forward analysis** — train/test split backtesting with overfitting detection
- **Backtest CSV export** (`--export`) — equity curves, per-strategy allocations, and fill logs
- **Yahoo Finance backtest** (`--yahoo RELIANCE.NS`) — fetch real Indian equity data for backtesting
- **Real pairs data backtest** (`--pairs-data ETHUSDT,BTCUSDT`) — fetch live klines for pairs backtesting
- **Tax-aware profitability** — India 30% crypto tax + 1% TDS, equity STCG/LTCG factored into entry decisions
- **Slippage protection** — reject adverse fills, convert market orders to aggressive limits
- **Private WebSocket** — instant fill detection via Bybit private WS (reduces latency vs polling)
- **`--venue zerodha` CLI flag** — run any mode (grid, orchestrator) against Indian markets
- **`access_token_file`** config field — daily Kite token auto-loaded from file written by login helper
- 300+ tests (up from 64)

## [2026.4.23]

### Added
- Grid trading engine for Bybit spot markets (V5 API)
- Auto-pilot mode: autonomous scan, deploy, monitor, and switch cycle
- Market intelligence: RSI, ADX, ATR, Bollinger Bands, SMA, EMA, VWAP
- Regime detection: Ranging, TrendingUp, TrendingDown, Volatile
- Kelly criterion position sizing with regime adjustment
- Risk engine: exposure limits, daily/weekly loss caps, max drawdown, kill switch
- Web dashboard at `http://localhost:9090` with live charts
- TUI terminal dashboard (`--tui`)
- Telegram bot for remote control (`/status`, `/pnl`, `/pause`, `/resume`)
- Webhook alerts (Slack, Discord, HTTP)
- Prometheus metrics endpoint (`/metrics`)
- Backtester with optimizer sweep
- Market scanner CLI
- SQLite persistence with WAL mode
- Circuit breaker with exponential backoff
- Token bucket rate limiting
- Docker multi-stage build
- Systemd service file
- GitHub Actions CI/CD with auto-release
- Dependabot for cargo and actions updates
- mdBook documentation site
- 64 unit tests across all modules
- Date-based auto-versioning via build.rs
