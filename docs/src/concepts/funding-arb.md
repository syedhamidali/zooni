# Funding Rate Arbitrage

Funding rate arbitrage is a delta-neutral carry trade on perpetual futures. When the perpetual funding rate is positive, long holders pay short holders every 8 hours. By holding a matched long spot position and short perpetual position, you collect funding while remaining neutral to price movements.

## How It Works

```
┌──────────┐     ┌──────────┐     ┌──────────┐     ┌──────────┐
│  Scan    │ ──> │  Score   │ ──> │  Enter   │ ──> │ Collect  │
│  Rates   │     │  & Rank  │     │  Hedge   │     │ Funding  │
└──────────┘     └──────────┘     └──────────┘     └────┬─────┘
     ^                                                   │
     │          Re-scan every N minutes                  │
     └───────────────────────────────────────────────────┘
```

### 1. Scan

Fetches all perpetual contract tickers from Bybit and pre-filters by:
- Current funding rate (above entry threshold)
- 24h turnover (liquidity filter)
- Mark price validity

### 2. Score & Rank

For each candidate, fetches historical funding rates and scores based on:
- **Funding rate** — higher rate = more yield
- **Consistency** — average rate vs current rate
- **Positive streak** — consecutive positive funding intervals
- **Liquidity** — 24h turnover (higher = safer entry/exit)

Symbols are ranked by composite score. Consistently positive funding with high liquidity scores highest.

### 3. Enter

For top-ranked symbols, enters a delta-neutral position:
1. **Buy spot** — market buy on the spot market
2. **Short perpetual** — market sell on the perpetual market (same quantity)

These two legs cancel out price risk. If the price goes up, the spot position gains while the perp position loses (and vice versa). Net exposure is zero.

> If the perpetual leg fails, the bot automatically unwinds the spot leg to avoid directional exposure.

### 4. Collect Funding

Every 8 hours (on Bybit), shorts receive a funding payment when the funding rate is positive. The bot monitors:
- Current funding rate — exit if it flips negative
- Unrealised P&L — exit if losses exceed the max loss threshold
- Position status — track accumulated funding

### 5. Exit

The bot closes both legs when:
- Funding rate drops below the exit threshold
- Unrealised loss exceeds `max_unrealised_loss_pct`
- A symbol disappears from the ticker list
- The kill switch is tripped (manual or risk engine)

## Profit Calculation

Funding payments accrue every 8 hours (3 times per day on Bybit):

```
daily_yield    = funding_rate * 3
annual_yield   = funding_rate * 3 * 365
```

For example, with a 0.01% funding rate per 8h:
- Daily yield: 0.03% per day
- Annualised yield: ~10.95% APY
- On a $10,000 position: ~$3/day or ~$1,095/year

Trading fees (entry + exit) are the main cost. At 0.06% taker fee per leg:
- Entry: 0.06% + 0.06% = 0.12%
- Exit: 0.06% + 0.06% = 0.12%
- Total round-trip: ~0.24%

Break-even after ~8 funding intervals (2.7 days) at 0.03%/day.

## Configuration

```toml
[funding_arb]
entry_threshold         = 0.0001     # min rate to enter (0.01% per 8h)
exit_threshold          = -0.00005   # exit when rate drops below this
min_turnover            = 5000000.0  # min 24h turnover in USDT
max_position_pct        = 0.15       # max 15% of equity per position
leverage                = 1          # perp leg leverage (1 = no leverage)
max_positions           = 3          # max simultaneous positions
min_positive_intervals  = 3          # min consecutive positive funding periods
max_unrealised_loss_pct = 0.02       # force-exit if unrealised loss > 2%
scan_interval_secs      = 300        # scan every 5 minutes
```

## Running

```bash
# Live trading
./zooni config.toml --funding-arb

# Dry-run mode (no orders sent)
./zooni config.toml --funding-arb --dry-run
```

## Key Parameters

### Entry Threshold

Higher thresholds are more selective — fewer trades but higher quality. The default `0.0001` (0.01%) equates to ~10.95% APY. During extreme market conditions, funding can reach 0.1% per interval (109.5% APY).

### Leverage

The perpetual leg can use leverage to amplify funding returns. Higher leverage increases yield but also increases liquidation risk if the perp-spot price diverges (basis risk). Default is `1` (no leverage) for safety.

### Max Positions

Running multiple positions diversifies funding income across assets. Each position uses up to `max_position_pct` of equity. With 3 positions at 15% each, maximum deployment is 45% of equity.

## When Funding Arb Works

| Market Condition | Funding Performance |
|-----------------|-------------------|
| Bull market (high leverage longs) | Excellent — funding rates stay positive for extended periods |
| Range-bound | Good — funding fluctuates but averages positive |
| Bear market (shorts dominate) | Poor — funding goes negative, shorts pay longs |
| Low volatility | Moderate — rates are small but consistent |
| Extreme events | Risky — basis can diverge, causing unrealised losses |

## Risks

- **Basis risk** — spot and perpetual prices can temporarily diverge, causing unrealised loss on one leg
- **Funding reversal** — rates can flip negative unexpectedly; the exit threshold limits exposure
- **Execution risk** — if one leg fills but the other doesn't, you have directional exposure (the bot auto-unwinds)
- **Exchange risk** — both positions must be on the same exchange (Bybit)

## Notes

- Metrics are served on port 9091 (separate from the grid bot's port 9090)
- All [risk management](./risk-management.md) limits still apply
- The strategy tag for risk tracking is `funding_arb`
- Active positions are unwound gracefully on shutdown
- Funding arb can run alongside the grid bot (different modes, different ports)
