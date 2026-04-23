# Web Dashboard

Zooni includes an embedded web dashboard that starts automatically with the bot. No external dependencies — the HTML/CSS/JS is compiled into the binary.

## Access

The dashboard runs at **`http://localhost:9090`** by default.

## Features

- **Stats Cards** — current price, equity, daily P&L, total P&L
- **Price Chart** — canvas-rendered price history with area fill
- **Equity Chart** — equity curve over time
- **Grid Visualization** — active grid levels with buy/sell indicators
- **Fills Table** — recent fills with side, price, quantity, and P&L
- **Regime Badge** — current market regime (Ranging, Trending, Volatile)
- **Auto-refresh** — updates every 2 seconds

## Design

- Dark theme optimized for monitoring
- No external JavaScript dependencies
- Custom `MiniChart` canvas library for lightweight rendering
- Responsive layout

## Endpoints

| Path | Content-Type | Description |
|------|-------------|-------------|
| `GET /` | `text/html` | Interactive dashboard |
| `GET /health` | `text/plain` | Health check (`"ok"`) |
| `GET /status` | `application/json` | Bot status snapshot |
| `GET /metrics` | `text/plain` | Prometheus exposition format |
| `GET /api/live` | `application/json` | Full live data for dashboard |

## `/api/live` Response

```json
{
  "price_history": [64500.0, 64520.0, ...],
  "equity_history": [10000.0, 10002.5, ...],
  "pnl_history": [0.0, 2.5, ...],
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
    { "price": 64000.0, "side": "Buy", "status": "open" },
    { "price": 65000.0, "side": "Sell", "status": "open" }
  ],
  "regime": "Ranging",
  "regime_details": "ADX: 18.2, RSI: 48.7"
}
```

## `/status` Response

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
