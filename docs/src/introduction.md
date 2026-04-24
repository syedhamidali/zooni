# Zooni

Zooni is an autonomous algorithmic trading bot built in Rust. It runs grid trading strategies on cryptocurrency exchanges, with built-in market intelligence to pick symbols, size positions, and switch between opportunities automatically.

## What Zooni Does

1. **Analyzes markets** using technical indicators (RSI, ADX, ATR, Bollinger Bands, VWAP) to detect whether a market is ranging, trending, or volatile
2. **Picks the best symbols** by scanning all available pairs and ranking them by grid-trading suitability
3. **Deploys grid orders** — layered buy and sell limit orders that capture profit from price oscillations
4. **Runs funding rate arbitrage** — delta-neutral carry trades that collect perpetual funding payments (long spot + short perp)
5. **Manages risk** with per-symbol exposure caps, daily/weekly loss limits, max drawdown protection, and a kill switch
6. **Monitors and adapts** — periodically re-scans the market and switches to better opportunities when they appear

## Key Properties

- **Written in Rust** — single binary, no runtime dependencies, minimal memory footprint
- **Persistent** — SQLite order journal survives restarts; state auto-recovers
- **Observable** — web dashboard, terminal TUI, Telegram bot, Prometheus metrics, webhook alerts
- **Safe by default** — testnet mode, dry-run mode, circuit breakers, rate limiting
- **Self-contained** — no external services required beyond the exchange API

## Binaries

| Binary | Purpose |
|--------|---------|
| `trading-bot` | Main bot — runs grids, autopilot, funding arb, dashboard, Telegram |
| `backtest` | Test strategies against historical or synthetic price data |
| `scan` | One-shot market scanner — rank symbols by grid suitability |

## Version

Zooni uses date-based versioning (`YYYY.M.D`). Check your version:

```bash
./trading-bot --version
# zooni 2026.4.23 (29db06d)
```

