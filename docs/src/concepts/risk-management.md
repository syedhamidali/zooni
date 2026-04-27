# Risk Management

Zooni enforces multiple layers of risk protection. Every order passes through the risk engine before reaching the exchange.

## Risk Controls

### Exposure Limits

Caps how much capital can be deployed at once:

| Limit | Default | Description |
|-------|---------|-------------|
| `max_total_exposure` | 80% | Maximum total portfolio exposure |
| `max_per_market` | 40% | Maximum exposure per market type |
| `max_per_symbol` | 20% | Maximum exposure per trading pair |
| `max_per_strategy` | 15% | Maximum exposure per strategy |

All values are fractions of `starting_equity_quote`.

### Loss Limits

Automatic pause when losses exceed thresholds:

| Limit | Default | What Happens |
|-------|---------|-------------|
| `daily_loss_limit` | 5% | Pauses new orders for the rest of the day |
| `weekly_loss_limit` | 10% | Pauses new orders for the rest of the week |
| `max_drawdown` | 15% | Pauses until equity recovers above threshold |

### Kill Switch

`Ctrl+C` triggers an atomic kill switch that immediately stops all trading. The bot will:
1. Cancel all open orders on the exchange
2. Wait for in-flight API calls (up to `shutdown_wait_secs`)
3. Save state to the database

### Circuit Breaker

After `circuit_breaker_threshold` consecutive API failures (default: 5), the bot pauses all API calls for `circuit_breaker_cooldown_secs` (default: 60). It auto-resets after the cooldown.

## Configuration

```toml
[risk]
starting_equity_quote = 10000.0   # your actual USDT balance
max_total_exposure    = 0.80
max_per_market        = 0.40
max_per_symbol        = 0.20
max_per_strategy      = 0.15
daily_loss_limit      = 0.05
weekly_loss_limit     = 0.10
max_drawdown          = 0.15
```

## How Pre-Trade Checks Work

Before every order placement:

1. **Kill switch** — is the bot shutting down?
2. **Daily P&L** — has the daily loss limit been hit?
3. **Weekly P&L** — has the weekly loss limit been hit?
4. **Drawdown** — is equity too far below peak?
5. **Symbol exposure** — would this order exceed the per-symbol cap?
6. **Total exposure** — would total exposure exceed the portfolio cap?

If any check fails, the order is rejected and the reason is logged. Webhook alerts fire for risk breaches if configured.

## P&L Tracking

All fills are recorded in the SQLite database with realized P&L. Query your performance:

```bash
# Daily P&L summary
sqlite3 zooni.db \
  "SELECT date(filled_at) as day, SUM(realised_pnl) as pnl, COUNT(*) as fills 
   FROM fills GROUP BY day ORDER BY day;"

# Total P&L
sqlite3 zooni.db \
  "SELECT SUM(realised_pnl) as total_pnl, COUNT(*) as total_fills FROM fills;"
```

## Regime-Aware Pausing

In auto-pilot mode, the bot also considers market regime. If the regime detector classifies the market as strongly trending, the bot can pause grid trading for that symbol and wait for conditions to improve. See [Market Intelligence](./market-intelligence.md).
