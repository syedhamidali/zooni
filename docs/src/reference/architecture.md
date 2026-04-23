# Architecture

## Source Layout

```
src/
├── main.rs                  Entry point, CLI args, --version
├── lib.rs                   Library crate module registry
├── bot.rs                   Core orchestration: startup → poll → shutdown
├── grid.rs                  Pure grid math (price levels, order generation)
├── autopilot.rs             Autonomous scan → deploy → switch cycle
├── config.rs                TOML config loader + validation
│
├── indicators.rs            Technical indicators (SMA, EMA, RSI, ADX, ATR, BB, VWAP)
├── regime.rs                Market regime detection + classification
├── scanner.rs               Market scanner — rank symbols by suitability
├── sizing.rs                Kelly criterion + regime-adjusted position sizing
│
├── risk.rs                  Pre-trade risk checks (exposure, loss, drawdown)
├── persistence.rs           SQLite order journal + P&L storage
├── retry.rs                 Exponential backoff + circuit breaker
├── rate_limit.rs            Token bucket rate limiter
├── market_hours.rs          Trading session windows (crypto/equity/commodity)
│
├── metrics.rs               Web dashboard + Prometheus metrics server
├── telegram.rs              Telegram bot for remote control
├── tui.rs                   Terminal UI dashboard (ratatui)
├── alerts.rs                Webhook notifications (Slack/Discord/HTTP)
├── ws_private.rs            Private WebSocket stream handler
├── types.rs                 Shared types (Side, OrderStatus, etc.)
│
├── strategy/
│   ├── mod.rs               Strategy trait + StrategyAction enum
│   └── dca.rs               Dollar-cost averaging strategy
│
├── venues/
│   ├── bybit/
│   │   ├── mod.rs           Bybit venue re-exports
│   │   ├── rest.rs          REST API client (V5, spot)
│   │   ├── ws.rs            WebSocket client
│   │   └── signer.rs        HMAC-SHA256 request signing
│   ├── hyperliquid/         (planned)
│   ├── zerodha/             (planned)
│   ├── interactive_brokers/ (planned)
│   └── mcx/                 (planned — via Indian brokers)
│
└── bin/
    ├── backtest.rs           Backtester CLI
    └── scan.rs               Market scanner CLI
```

## Data Flow

```
                    ┌──────────────┐
                    │  Exchange    │
                    │  (Bybit V5)  │
                    └──────┬───────┘
                           │ REST API / WebSocket
                    ┌──────┴───────┐
                    │ Rate Limiter │
                    │ + Retry      │
                    │ + Circuit    │
                    │   Breaker    │
                    └──────┬───────┘
                           │
        ┌──────────────────┼──────────────────┐
        │                  │                  │
  ┌─────┴─────┐     ┌─────┴─────┐     ┌─────┴─────┐
  │  Scanner  │     │   Grid    │     │  Metrics  │
  │ + Regime  │     │  Engine   │     │ + Dashboard│
  │ + Sizing  │     │           │     │           │
  └─────┬─────┘     └─────┬─────┘     └───────────┘
        │                  │
  ┌─────┴─────┐     ┌─────┴─────┐
  │ Autopilot │     │   Risk    │
  │ (optional)│────>│  Engine   │
  └───────────┘     └─────┬─────┘
                          │
                    ┌─────┴─────┐
                    │  SQLite   │
                    │ Persistence│
                    └───────────┘
```

## Key Design Decisions

### Single Binary

All functionality (bot, dashboard, Telegram, metrics) runs in one process. No microservices, no IPC, no message queues. This simplifies deployment and reduces failure modes.

### Pure Grid Math

`grid.rs` is pure computation — no I/O, no async. It takes a price range and levels, returns order prices. This makes it testable and reusable across live trading, backtesting, and auto-pilot.

### Atomic State

Risk engine state, circuit breaker counters, and kill switch use `AtomicU64`/`AtomicBool`. No locks needed for the hot path.

### Fire-and-Forget Alerts

Webhook delivery never blocks trading. Failures are logged and ignored.

### SQLite with WAL

The database uses WAL (Write-Ahead Logging) mode for concurrent reads during writes. One writer (the bot), many readers (dashboard queries, CLI tools).

## Concurrency Model

```
Main thread (tokio runtime)
├── Bot poll loop (grid reconciliation every poll_secs)
├── Metrics HTTP server (axum, port 9090)
├── Telegram poller (every 5s)
├── Regime detector (every ~5 min)
└── Auto-pilot scanner (every scan_interval_secs)
```

All tasks run on the same tokio runtime. Shared state (metrics, TUI state) is accessed via `Arc<Mutex<_>>` or `Arc<Atomic*>`.
