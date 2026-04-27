# Configuration Reference

Complete reference for all fields in `config.toml`.

## `[bot]`

General bot settings.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `dry_run` | bool | `false` | Simulate orders without exchange calls |
| `db_path` | string | `"zooni.db"` | SQLite database file path |
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

## `[zerodha]`

Zerodha Kite Connect v3 credentials. Optional — omit to disable the Zerodha venue.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `api_key` | string | *required* | Kite Connect API key |
| `api_secret` | string | *required* | Kite Connect API secret |
| `access_token` | string | `""` | Session token (refreshed daily via login flow) |

### Authentication Flow

1. Call `login_url()` to get the Kite login page URL
2. User logs in and is redirected with a `request_token`
3. Call `exchange_request_token()` to swap for an `access_token`
4. The access token is valid for one trading day

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

## `[funding_arb]`

Funding rate arbitrage settings. Optional — run with `--funding-arb` flag.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `entry_threshold` | f64 | `0.0001` | Min funding rate to enter a position (0.01% per 8h) |
| `exit_threshold` | f64 | `-0.00005` | Exit when funding rate drops below this |
| `min_turnover` | f64 | `5000000.0` | Min 24h turnover in USDT (liquidity filter) |
| `max_position_pct` | f64 | `0.15` | Max fraction of equity per position |
| `leverage` | u32 | `1` | Perpetual leg leverage (1 = no leverage) |
| `max_positions` | usize | `3` | Max simultaneous arb positions |
| `min_positive_intervals` | usize | `3` | Min consecutive positive funding periods to enter |
| `max_unrealised_loss_pct` | f64 | `0.02` | Force-exit if unrealised loss exceeds 2% |
| `scan_interval_secs` | u64 | `300` | Seconds between rate scans (5 minutes) |

## `[tax]`

Tax-aware profitability engine. Optional — defaults to `jurisdiction = "none"` (no tax checks).

Set `jurisdiction = "india"` for Indian market trading so the bot only enters trades that are profitable after all deductions.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `jurisdiction` | string | `"none"` | `"india"` or `"none"` |
| `crypto_tax_rate` | f64 | `0.30` | Flat tax on crypto gains (30% in India) |
| `crypto_tds_rate` | f64 | `0.01` | TDS on every crypto sale (1% in India) |
| `equity_stcg_rate` | f64 | `0.15` | Short-term capital gains on equity (15%) |
| `equity_ltcg_rate` | f64 | `0.10` | Long-term capital gains on equity (10%) |
| `trading_fee_rate` | f64 | `0.001` | Brokerage + exchange fees per leg |

### Break-even Thresholds (India defaults)

| Asset Class | Min Profit to Break Even |
|-------------|--------------------------|
| Indian crypto | ~1.76% (30% tax + 1% TDS + fees) |
| Indian equity | ~0.24% (15% STCG + fees) |
| International crypto | ~0.20% (fees only) |

## `[slippage]`

Slippage protection. Optional — sensible defaults applied automatically.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `max_slippage_pct` | f64 | `0.005` | Reject fills with > 0.5% slippage |
| `prefer_limit_orders` | bool | `true` | Convert market orders to aggressive limits |
| `limit_offset_pct` | f64 | `0.001` | Aggressive limit offset from current price (0.1%) |

## Validation Rules

- `grid.levels` must be >= 2 (unless autopilot is enabled)
- `grid.lower_price` must be > 0 and < `grid.upper_price`
- `grid.qty_per_level_base` must be > 0
- `bybit.api_key` and `api_secret` must be non-empty unless `bot.dry_run = true`
- `risk.starting_equity_quote` must be > 0
