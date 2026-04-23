# Alerts & Webhooks

Zooni can send notifications via webhook to Slack, Discord, or any HTTP POST endpoint.

## Setup

Add a webhook URL to your config:

```toml
[alerts]
webhook_url = "https://hooks.slack.com/services/T.../B.../..."
```

## Event Types

| Event | Config Flag | Default | Alert Level |
|-------|------------|---------|-------------|
| Order fill | `on_fill` | `false` | Info |
| Risk breach | `on_risk_breach` | `true` | Critical |
| Bot start/stop | `on_lifecycle` | `true` | Info |
| API error | `on_error` | `true` | Warning |
| P&L summary | `pnl_summary_interval_secs` | `0` (disabled) | Info |

## Configuration

```toml
[alerts]
webhook_url = "https://hooks.slack.com/services/..."
on_fill = false                  # fills can be noisy on tight grids
on_risk_breach = true            # always know when limits are hit
on_lifecycle = true              # startup/shutdown notifications
on_error = true                  # API failures, circuit breaker trips
pnl_summary_interval_secs = 3600 # hourly P&L digest (0 = disabled)
```

## Message Format

Alerts are sent as Slack-compatible JSON:

```json
{"text": "ℹ️ *[zooni]* `lifecycle` — Bot started on BTCUSDT (testnet, dry-run)"}
```

### Alert Levels

| Level | Prefix | Used For |
|-------|--------|----------|
| Info | ℹ️ | Fills, lifecycle, P&L summaries |
| Warning | ⚠️ | API errors, circuit breaker |
| Critical | 🚨 | Risk limit breaches |

## Webhook Compatibility

The payload uses Slack's `{"text": "..."}` format, which is also compatible with:

- **Slack** — Incoming Webhooks
- **Discord** — Webhook URLs (via Slack-compatible endpoint)
- **Any HTTP service** — that accepts POST with JSON body

## Fire-and-Forget

Alert delivery is non-blocking. If the webhook fails (network error, timeout), it's logged but never blocks or crashes the trading logic. The webhook has a 10-second timeout.
