# API Endpoints

The bot runs an HTTP server on port 9090 with the following endpoints.

## `GET /`

Returns the interactive HTML dashboard. Dark-themed, auto-refreshing, with charts and grid visualization.

**Content-Type:** `text/html`

## `GET /health`

Simple health check.

**Content-Type:** `text/plain`

**Response:** `ok`

## `GET /status`

Bot status as JSON. Useful for external monitoring scripts.

**Content-Type:** `application/json`

**Response:**

```json
{
  "orders_placed": 42,
  "orders_filled": 28,
  "orders_failed": 0,
  "daily_pnl": 12.50,
  "total_pnl": 45.30,
  "equity": 10045.30,
  "uptime_secs": 3600
}
```

## `GET /metrics`

Prometheus exposition format. Scrape this endpoint with Prometheus for time-series monitoring.

**Content-Type:** `text/plain`

**Response:**

```
# HELP orders_placed Total orders placed
# TYPE orders_placed counter
orders_placed 42

# HELP orders_filled Total orders filled
# TYPE orders_filled counter
orders_filled 28

# HELP orders_failed Total orders failed
# TYPE orders_failed counter
orders_failed 0

# HELP daily_pnl Daily realized PnL
# TYPE daily_pnl gauge
daily_pnl 12.50

# HELP total_pnl Total realized PnL
# TYPE total_pnl gauge
total_pnl 45.30

# HELP equity Current equity
# TYPE equity gauge
equity 10045.30

# HELP uptime_seconds Bot uptime
# TYPE uptime_seconds counter
uptime_seconds 3600
```

## `GET /api/live`

Full live data for the dashboard. Includes time-series history, grid state, and regime info.

**Content-Type:** `application/json`

**Response:**

```json
{
  "price_history": [64500.0, 64520.0, 64480.0],
  "equity_history": [10000.0, 10002.5, 10005.0],
  "pnl_history": [0.0, 2.5, 5.0],
  "recent_fills": [
    {
      "time": "2026-04-23T10:15:00Z",
      "symbol": "BTCUSDT",
      "side": "Buy",
      "price": 64500.0,
      "qty": 0.001,
      "pnl": 0.5
    }
  ],
  "grid_levels": [
    {"price": 64000.0, "side": "Buy", "status": "open"},
    {"price": 65000.0, "side": "Sell", "status": "open"}
  ],
  "regime": "Ranging",
  "regime_details": "ADX: 18.2, RSI: 48.7"
}
```

## Integration Examples

### Curl

```bash
# Health check
curl http://localhost:9090/health

# JSON status
curl -s http://localhost:9090/status | jq .

# P&L only
curl -s http://localhost:9090/status | jq '{daily: .daily_pnl, total: .total_pnl}'
```

### Monitoring Script

```bash
#!/bin/bash
PNL=$(curl -s http://localhost:9090/status | jq .daily_pnl)
if (( $(echo "$PNL < -100" | bc -l) )); then
  echo "ALERT: Daily PnL is $PNL" | mail -s "Zooni Alert" you@example.com
fi
```
