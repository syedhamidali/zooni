# Changelog

All notable changes to Zooni will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Zooni uses date-based versioning: `YYYY.M.D`.

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
