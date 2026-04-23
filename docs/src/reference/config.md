# Configuration Reference

Complete reference for all fields in `config.toml`.

## `[bot]`

General bot settings.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `dry_run` | bool | `false` | Simulate orders without exchange calls |
| `db_path` | string | `"trading-bot.db"` | SQLite database file path |
| `max_retries` | u32 | `3` | Max retry attempts per API call |
| `circuit_breaker_threshold` | u32 | `5` | Consecutive failures before circuit break |
| `circuit_breaker_cooldown_secs` | u64 | `60` | Seconds to pause after circuit break |
| `shutdown_wait_secs` | u64 | `10` | Seconds to wait for in-flight orders at shutdown |

## `[bybit]`

Exchange API credentials and connection settings.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `api_key` | string | *required* | Bybit API key (empty string OK in dry-run) |
| `api_secret` | string | *required* | Bybit API secret |
| `testnet` | bool | `true` | Use testnet endpoints |
| `recv_window_ms` | u64 | `5000` | Request receive window (milliseconds) |
| `account_type` | string | `"UNIFIED"` | Account type for wallet balance lookups |

## `[grid]`

Grid trading strategy parameters. Ignored when `autopilot.enabled = true`.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `symbol` | string | *required* | Trading pair (e.g., `"BTCUSDT"`) |
| `lower_price` | f64 | *required* | Grid lower price bound |
| `upper_price` | f64 | *required* | Grid upper price bound (must be > lower) |
| `levels` | u32 | *required* | Number of grid lines (must be >= 2) |
| `qty_per_level_base` | f64 | *required* | Base asset quantity per grid order |
| `poll_secs` | u64 | `3` | Seconds between fill-detection polls |
| `cancel_existing_on_start` | bool | `true` | Cancel all existing orders on symbol at startup |
| `auto_rebalance` | bool | `false` | Shift grid when price exits the range |
| `trailing_profit_threshold` | f64 | `0.0` | Take-profit threshold in quote currency (0 = disabled) |

## `[risk]`

Risk management limits. All fraction values are relative to `starting_equity_quote`.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `starting_equity_quote` | f64 | *required* | Starting balance in quote currency (USDT) |
| `max_total_exposure` | f64 | `0.80` | Max total portfolio exposure (80%) |
| `max_per_market` | f64 | `0.40` | Max exposure per market type |
| `max_per_symbol` | f64 | `0.20` | Max exposure per trading pair |
| `max_per_strategy` | f64 | `0.15` | Max exposure per strategy |
| `daily_loss_limit` | f64 | `0.05` | Daily loss limit (5%) |
| `weekly_loss_limit` | f64 | `0.10` | Weekly loss limit (10%) |
| `max_drawdown` | f64 | `0.15` | Max drawdown from equity peak (15%) |

## `[alerts]`

Webhook notification settings. Omit the entire section to disable alerts.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `webhook_url` | string? | `null` | Webhook URL (Slack, Discord, or HTTP POST) |
| `on_fill` | bool | `false` | Send alert on every order fill |
| `on_risk_breach` | bool | `true` | Alert when risk limits are hit |
| `on_lifecycle` | bool | `true` | Alert on bot startup/shutdown |
| `on_error` | bool | `true` | Alert on API errors and circuit breaker |
| `pnl_summary_interval_secs` | u64 | `0` | Periodic P&L summary interval (0 = disabled) |

## `[autopilot]`

Autonomous trading mode. When enabled, `[grid]` symbol/range are ignored.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable auto-pilot mode |
| `scan_interval_secs` | u64 | `1800` | Re-scan market every N seconds (30 min) |
| `switch_threshold` | f64 | `15.0` | Score improvement needed to switch symbols |
| `max_grids` | usize | `1` | Max simultaneous grids (symbols) |

## `[autopilot.scanner]`

Scanner tuning for auto-pilot market analysis.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `min_turnover` | f64 | `1000000.0` | Minimum 24h turnover in quote currency |
| `max_spread_pct` | f64 | `0.2` | Maximum bid-ask spread percentage |
| `quote_currencies` | string[] | `["USDT"]` | Quote currencies to scan |
| `max_results` | usize | `10` | Top N candidates to analyze in depth |
| `kline_interval` | string | `"60"` | Kline interval in minutes |
| `kline_count` | usize | `200` | Number of candles to fetch per symbol |

## Validation Rules

- `grid.levels` must be >= 2 (unless autopilot is enabled)
- `grid.lower_price` must be > 0 and < `grid.upper_price`
- `grid.qty_per_level_base` must be > 0
- `bybit.api_key` and `api_secret` must be non-empty unless `bot.dry_run = true`
- `risk.starting_equity_quote` must be > 0
