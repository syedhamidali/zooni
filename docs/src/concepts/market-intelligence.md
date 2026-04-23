# Market Intelligence

Zooni uses technical indicators and regime detection to evaluate whether a market is suitable for grid trading, and to size positions appropriately.

## Technical Indicators

| Indicator | Function | Used For |
|-----------|----------|----------|
| **SMA** | Simple Moving Average | Trend direction baseline |
| **EMA** | Exponential Moving Average | Responsive trend detection |
| **RSI** | Relative Strength Index (0-100) | Overbought/oversold detection |
| **ADX** | Average Directional Index (0-100) | Trend strength measurement |
| **ATR** | Average True Range | Volatility measurement |
| **Bollinger Bands** | Price envelope (upper, mid, lower) | Range/breakout detection |
| **VWAP** | Volume-Weighted Average Price | Fair value reference |

## Regime Detection

The regime detector classifies the market into four states based on indicator values:

| Regime | Conditions | Grid Suitability |
|--------|-----------|-----------------|
| **Ranging** | Low ADX (< 25), neutral RSI, tight BB | Best — frequent oscillations |
| **Trending Up** | High ADX (> 25), RSI > 60 | Poor — buys fill, sells don't |
| **Trending Down** | High ADX (> 25), RSI < 40 | Poor — sells fill, buys don't |
| **Volatile** | Wide Bollinger bandwidth (> 20%) | Risky — large swings |

### Classification Logic

```
if bollinger_bandwidth > 0.20 → Volatile
else if adx > 25 and rsi > 60 → Trending Up
else if adx > 25 and rsi < 40 → Trending Down
else → Ranging
```

### Regime Analysis Output

Each analysis produces:

- **Regime classification** — Ranging, TrendingUp, TrendingDown, Volatile
- **Grid suitability flag** — true if conditions favor grid trading
- **Suggested price range** — derived from Bollinger Bands (lower to upper)
- **Suggested levels** — based on ATR and range width
- **Volatility percentage** — ATR as a percentage of price

## Scoring

The scanner combines regime analysis into a composite score for ranking symbols:

- **Ranging markets** score highest
- **Low volatility** within range is preferred
- **High 24h turnover** indicates good liquidity
- **Tight bid-ask spread** means lower execution cost

## How It's Used

### Manual Mode
The bot runs regime detection every ~5 minutes and can auto-pause if conditions become unfavorable (trending or volatile).

### Auto-Pilot Mode
The scanner uses regime detection to rank all available symbols and deploy grids on the best candidates. See [Auto-Pilot Mode](./autopilot.md).

### Backtester
The backtester can use ATR-based adaptive sizing to adjust position size with volatility. See [Backtester](../tools/backtester.md).

## Position Sizing

The sizing engine blends two approaches:

1. **Kelly Criterion** — optimal bet size based on win rate and payoff ratio
2. **Regime Adjustment** — scales position size based on market conditions

```
kelly_fraction = (win_rate * avg_win - (1 - win_rate) * avg_loss) / avg_win
adjusted_size = base_size * kelly_blend * regime_factor * performance_factor
```

The performance factor dynamically scales up during profitable streaks and down during losing streaks, providing natural risk adjustment.
