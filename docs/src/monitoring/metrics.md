# Prometheus Metrics

Zooni exposes metrics in Prometheus exposition format for integration with Grafana or other monitoring stacks.

## Endpoint

```
GET http://localhost:9090/metrics
```

## Available Metrics

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

# HELP daily_pnl Daily realized P&L (USDT)
# TYPE daily_pnl gauge
daily_pnl 12.50

# HELP total_pnl Total realized P&L (USDT)
# TYPE total_pnl gauge
total_pnl 45.30

# HELP equity Current equity (USDT)
# TYPE equity gauge
equity 10045.30

# HELP uptime_seconds Bot uptime in seconds
# TYPE uptime_seconds counter
uptime_seconds 3600
```

## Grafana Setup

1. Add Prometheus as a data source pointing to `http://<bot-host>:9090/metrics`
2. Create dashboards for:
   - P&L over time (`daily_pnl`, `total_pnl`)
   - Order flow (`orders_placed`, `orders_filled`, `orders_failed`)
   - Equity curve (`equity`)
   - Uptime (`uptime_seconds`)

## Alertmanager

Configure Prometheus Alertmanager rules for:

```yaml
- alert: HighFailureRate
  expr: orders_failed / orders_placed > 0.1
  for: 5m
  annotations:
    summary: "Order failure rate above 10%"

- alert: NegativeDailyPnL
  expr: daily_pnl < -100
  for: 1m
  annotations:
    summary: "Daily P&L below -100 USDT"
```
