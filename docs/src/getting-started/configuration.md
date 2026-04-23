# Configuration

Zooni is configured via a single TOML file. Copy the example and edit:

```bash
cp config.example.toml config.toml
```

> **Never commit `config.toml`** — it contains API secrets. It is in `.gitignore`.

## Minimal Config

The smallest working config for testnet dry-run:

```toml
[bot]
dry_run = true

[bybit]
api_key = ""
api_secret = ""
testnet = true

[grid]
symbol = "BTCUSDT"
lower_price = 60000.0
upper_price = 70000.0
levels = 10
qty_per_level_base = 0.001

[risk]
starting_equity_quote = 10000.0
```

## Config Sections

| Section | Purpose |
|---------|---------|
| `[bot]` | Dry-run, database path, retry/circuit-breaker settings |
| `[bybit]` | Exchange API credentials and endpoint settings |
| `[grid]` | Grid strategy parameters (symbol, range, levels, qty) |
| `[risk]` | Exposure limits, loss caps, drawdown thresholds |
| `[alerts]` | Webhook URL and notification filters |
| `[autopilot]` | Autonomous mode settings |
| `[autopilot.scanner]` | Market scanner tuning |

See the [Configuration Reference](../reference/config.md) for a complete field-by-field listing.

## Environment Variables

| Variable | Purpose |
|----------|---------|
| `RUST_LOG` | Log verbosity (e.g., `info`, `debug`, `trading_bot=debug,reqwest=warn`) |
| `TELEGRAM_BOT_TOKEN` | Telegram bot token for remote control |
| `TELEGRAM_CHAT_ID` | Telegram chat ID for notifications |

## Security Notes

- API keys are stored in plain text in `config.toml`. Protect file permissions: `chmod 600 config.toml`
- On a VPS, consider using environment variables or a secrets manager instead of a config file
- The `.gitignore` blocks `config.toml`, `.env`, `*.key`, `*.pem`, and `*.secret`
