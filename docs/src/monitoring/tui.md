# TUI Terminal Dashboard

Zooni includes a terminal-based UI built with [ratatui](https://ratatui.rs/) for monitoring directly in your terminal.

## Usage

```bash
./trading-bot config.toml --tui
```

## Display

The TUI shows:

- **Header** — bot name, symbol, mode (live/dry-run), uptime
- **Grid Status** — current grid levels with buy/sell prices
- **Current Price** — live mid-price
- **P&L** — daily and total realized P&L
- **Fill Count** — number of fills in current session
- **Open Orders** — count of active orders on the exchange
- **Circuit Breaker** — status (closed/open) and failure count

## Controls

| Key | Action |
|-----|--------|
| `q` | Quit (graceful shutdown) |
| `Ctrl+C` | Force quit |

## Notes

- The TUI disables the standard log output to avoid display corruption. Logs are still written if a file appender is configured.
- The TUI shares the same trading state as the bot — it's not a separate process.
- For remote monitoring, use the [Web Dashboard](./dashboard.md) or [Telegram Bot](./telegram.md) instead.
