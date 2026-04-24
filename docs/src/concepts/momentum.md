# Momentum & Mean Reversion

A dual-mode strategy that automatically switches between momentum trading and mean-reversion based on market regime detection. Designed for hourly/daily timeframes — suitable for Indian equities (SEBI-compliant, under 10 OPS) and crypto.

## How It Works

```
┌──────────┐     ┌──────────┐     ┌──────────┐     ┌──────────┐
│  Collect  │ ──> │  Detect  │ ──> │ Generate │ ──> │  Manage  │
│  Prices   │     │  Regime  │     │  Signal  │     │ Position │
└──────────┘     └──────────┘     └──────────┘     └────┬─────┘
     ^                                                   │
     │              Tick every poll interval              │
     └───────────────────────────────────────────────────┘
```

### 1. Collect Prices

Each tick, the strategy records the latest price into a rolling 200-bar history window. This feeds all technical indicators: RSI, EMA, Bollinger Bands, ATR, ADX.

### 2. Detect Regime

The strategy uses ADX (Average Directional Index) to classify the current market:

| ADX Value | Regime | Strategy Used |
|-----------|--------|---------------|
| > 25 | Trending | Momentum |
| < 25 | Ranging | Mean Reversion |

In **Auto** mode (default), the strategy switches seamlessly. You can also force one mode via config.

### 3. Generate Signal

**Momentum mode** (trending markets):
- **Go Long**: EMA(9) crosses above EMA(21) AND RSI > 50 AND RSI < 70
- **Go Short**: EMA(9) crosses below EMA(21) AND RSI < 50 AND RSI > 30
- RSI filters prevent chasing overbought/oversold extremes

**Mean-reversion mode** (ranging markets):
- **Go Long**: Price touches lower Bollinger Band AND RSI < 30 (oversold)
- **Go Short**: Price touches upper Bollinger Band AND RSI > 70 (overbought)
- Profits from price returning to the mean

### 4. Manage Position

On entry:
- Places an aggressive limit order (at ask for buys, bid for sells)
- Sets an ATR-based stop loss (2× ATR from entry by default)
- Sets an ATR-based take profit (3× ATR from entry by default)

On exit:
- Stop loss → market order (fast exit to limit damage)
- Take profit → limit order (capture the price you want)
- Cooldown enforced between trades to prevent rapid-fire entries

## Risk Controls

| Control | Default | Purpose |
|---------|---------|---------|
| ATR stop loss | 2× ATR | Adaptive — widens in volatile markets, tightens in calm |
| ATR take profit | 3× ATR | Positive risk/reward ratio (1:1.5) |
| Max positions | 1 | Prevents over-exposure |
| Cooldown | 5 ticks | Prevents whipsawing on noisy signals |
| RSI filters | 30/70 | Prevents buying overbought or selling oversold |

## Configuration

```toml
# Momentum config is set programmatically (not in TOML yet).
# Key parameters:
symbol              = "NIFTY"
mode                = "auto"       # "auto", "momentum_only", "mean_reversion_only"
position_size_quote = 1000.0       # quote currency per trade
rsi_period          = 14
ema_fast            = 9
ema_slow            = 21
rsi_overbought      = 70.0
rsi_oversold        = 30.0
bb_period           = 20
bb_std              = 2.0
atr_period          = 14
stop_loss_atr_mult  = 2.0
take_profit_atr_mult = 3.0
max_open_positions  = 1
cooldown_ticks      = 5
```

## When Each Mode Works

| Market | Momentum | Mean Reversion |
|--------|----------|----------------|
| Strong trend (ADX > 30) | Excellent — rides the trend | Poor — fights the trend |
| Mild trend (ADX 20-25) | Moderate | Moderate |
| Range-bound (ADX < 20) | Poor — whipsawed by noise | Excellent — buys low, sells high |
| Volatile breakout | Good if direction is clear | Risky — bands widen, signals fire late |

## Tax Awareness

When trading Indian markets, the bot can factor in:
- **30% crypto tax** + 1% TDS on every sale — the bot won't enter trades that are unprofitable after tax
- **15% STCG** for equity — factored into minimum profit thresholds

See the `[tax]` config section for details.

## SEBI Compliance

For Indian equities:
- The bot rate-limits to **< 10 orders per second** (SEBI OPS threshold)
- Uses hourly/daily timeframes — not high-frequency
- All orders go through the Zerodha Kite Connect API with required daily authentication
