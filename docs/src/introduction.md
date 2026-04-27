# Zooni

Zooni is an autonomous algorithmic trading bot built in Rust. It runs multiple trading strategies across crypto and Indian equity markets, with built-in market intelligence, tax awareness, and risk management to trade profitably without human intervention.

## What Zooni Does

1. **Analyzes markets** using technical indicators (RSI, ADX, ATR, Bollinger Bands, VWAP) to detect whether a market is ranging, trending, or volatile
2. **Picks the best symbols** by scanning all available pairs and ranking them by grid-trading suitability
3. **Deploys grid orders** — layered buy and sell limit orders that capture profit from price oscillations
4. **Runs funding rate arbitrage** — delta-neutral carry trades that collect perpetual funding payments (long spot + short perp)
5. **Trades pairs** — statistical arbitrage on correlated stocks (e.g., HDFC/ICICI), profiting from spread reversion while staying market-neutral
6. **Follows momentum** — EMA crossover in trends, Bollinger Band reversion in ranges, with automatic regime switching
7. **Manages risk** with per-symbol exposure caps, daily/weekly loss limits, max drawdown protection, consecutive-loss circuit breaker, and a kill switch
8. **Monitors and adapts** — periodically re-scans the market and switches to better opportunities when they appear

## Key Properties

- **Written in Rust** — single binary, no runtime dependencies, minimal memory footprint
- **Persistent** — SQLite order journal survives restarts; state auto-recovers
- **Observable** — web dashboard, terminal TUI, Telegram bot, Prometheus metrics, webhook alerts
- **Safe by default** — testnet mode, dry-run mode, circuit breakers, rate limiting
- **Self-contained** — no external services required beyond the exchange API

## Binaries

| Binary | Purpose |
|--------|---------|
| `zooni` | Main bot — runs grids, autopilot, funding arb, dashboard, Telegram |
| `backtest` | Test strategies against historical or synthetic price data |
| `scan` | One-shot market scanner — rank symbols by grid suitability |

## Version

Zooni uses date-based versioning (`YYYY.M.D`). Check your version:

```bash
./zooni --version
# zooni 2026.4.23 (29db06d)
```

