# Pairs Trading (Statistical Arbitrage)

Pairs trading is a market-neutral strategy that profits from the relative price movement between two correlated instruments. It doesn't care whether the overall market goes up or down — it only trades the spread between the pair.

## How It Works

```
┌──────────┐     ┌──────────┐     ┌──────────┐     ┌──────────┐
│  Track   │ ──> │ Compute  │ ──> │  Check   │ ──> │ Execute  │
│  Prices  │     │  Spread  │     │ Z-Score  │     │   Pair   │
└──────────┘     └──────────┘     └──────────┘     └────┬─────┘
     ^                                                   │
     │              Tick every poll interval              │
     └───────────────────────────────────────────────────┘
```

### 1. Track Prices

The strategy monitors two correlated instruments simultaneously (e.g., HDFC Bank and ICICI Bank). It maintains a rolling price window for each and updates the hedge ratio.

### 2. Compute Spread

The spread is calculated as:

```
spread = log(price_A) - hedge_ratio × log(price_B)
```

The hedge ratio ensures the two legs are balanced. It can be:
- **Fixed** — a constant ratio you set manually
- **Rolling** — dynamically computed as `cov(A,B) / var(B)` from recent data

### 3. Check Z-Score

The z-score measures how far the current spread has deviated from its historical mean:

```
z_score = (current_spread - mean_spread) / std_dev_spread
```

| Z-Score | Meaning |
|---------|---------|
| 0 | Spread is at its historical average |
| +2 | Spread is 2σ above normal — A is expensive relative to B |
| -2 | Spread is 2σ below normal — A is cheap relative to B |
| ±3.5 | Extreme divergence — stop out |

### 4. Execute Pair

**Entry** (z-score crosses ±2.0):
- **Z > +2.0**: Spread too high → short A, buy B (expect A to drop or B to rise)
- **Z < -2.0**: Spread too low → buy A, short B (expect A to rise or B to drop)

**Exit** (z-score returns to ±0.5):
- Spread has reverted to near its mean → close both legs, take profit

**Stop** (z-score exceeds ±3.5):
- Spread diverged too far — the correlation may have broken → market-order close both legs

## Why It's Market-Neutral

If the entire market crashes 10%, both HDFC Bank and ICICI Bank drop roughly equally. The long leg loses, the short leg gains, and the net exposure is approximately zero. Profit comes only from the relative movement between the two.

## Best Pairs for Indian Markets

| Pair | Sector | Typical Correlation |
|------|--------|-------------------|
| HDFCBANK / ICICIBANK | Banking | > 0.90 |
| TCS / INFOSYS | IT Services | > 0.85 |
| RELIANCE / ONGC | Energy | > 0.75 |
| BHARTIARTL / JIO | Telecom | > 0.80 |
| SBIN / PNB | PSU Banking | > 0.85 |

## Configuration

```toml
[pairs]
symbol_a             = "NSE:HDFCBANK"
symbol_b             = "NSE:ICICIBANK"
lookback             = 100         # bars for spread statistics
entry_z_score        = 2.0         # enter when spread deviates > 2σ
exit_z_score         = 0.5         # exit when spread reverts within 0.5σ
stop_z_score         = 3.5         # stop out if spread diverges > 3.5σ
position_size_quote  = 50000.0     # ₹ per leg
max_open_pairs       = 1
```

## Key Parameters

### Entry Z-Score

Higher values are more selective — fewer trades but higher probability of mean reversion. The default `2.0` means the spread must be 2 standard deviations from its mean to enter. During normal market conditions, this fires a few times per week.

### Stop Z-Score

If the spread reaches 3.5σ, the historical correlation may have structurally broken (e.g., a merger announcement, sector rotation). The stop loss cuts the position to prevent unlimited losses.

### Lookback

Longer lookback (200+) gives more stable statistics but adapts slowly to regime changes. Shorter lookback (50) adapts faster but produces noisier signals.

## Risks

- **Correlation breakdown** — structural events (mergers, delistings, regulatory changes) can permanently break a pair's correlation
- **Basis risk** — short-term divergence can exceed the stop before reverting
- **Execution risk** — both legs must fill simultaneously; if one fails, you have directional exposure
- **Sector-wide events** — a sector-specific shock affects both stocks similarly, reducing the edge

## Notes

- Pairs trading works best on highly liquid stocks with tight spreads
- The strategy requires short-selling capability (F&O segment on NSE)
- All [risk management](./risk-management.md) limits still apply
- The strategy tag for risk tracking is `pairs`
- Tax-aware mode applies: 15% STCG on profitable trades (see `[tax]` config)
