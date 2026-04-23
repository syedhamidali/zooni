# Auto-Pilot Mode

Auto-pilot is Zooni's fully autonomous mode. Instead of trading a single pre-configured symbol, the bot scans the entire market, picks the best opportunities, deploys grids, and switches when conditions change.

## How It Works

```
┌─────────┐     ┌──────────┐     ┌──────────┐     ┌───────────┐
│  Scan   │ ──> │  Score   │ ──> │  Deploy  │ ──> │  Trade    │
│ Market  │     │ & Rank   │     │  Grid    │     │  Loop     │
└─────────┘     └──────────┘     └──────────┘     └─────┬─────┘
     ^                                                    │
     │              Re-scan every N minutes               │
     └────────────────────────────────────────────────────┘
```

### 1. Scan

Fetches all trading pairs from the exchange and pre-filters by:
- Minimum 24h turnover (liquidity threshold)
- Maximum bid-ask spread percentage
- Quote currency (default: USDT)

### 2. Score & Rank

For each candidate, fetches historical klines and runs:
- Regime detection (ADX, RSI, Bollinger Bands)
- Volatility analysis (ATR as % of price)
- Grid suitability scoring

Symbols are ranked by composite score. Ranging markets with moderate volatility and high liquidity score highest.

### 3. Deploy

For the top-ranked symbol(s):
- Calculates optimal grid range from Bollinger Bands
- Determines position size using Kelly criterion + regime adjustment
- Places the grid orders

### 4. Trade Loop

Standard grid trading cycle (poll for fills, place replacements). Periodically re-scans and evaluates:

- If current symbol's score drops significantly
- If a new symbol scores much higher (above `switch_threshold`)
- If current grid is losing money

### 5. Switch

When a better opportunity is found:
1. Cancel existing grid orders
2. Wait for in-flight orders to settle
3. Deploy new grid on the better symbol

A hysteresis threshold (`switch_threshold = 15.0` by default) prevents excessive switching.

## Configuration

```toml
[autopilot]
enabled = true
scan_interval_secs = 1800     # re-scan every 30 minutes
switch_threshold = 15.0       # score improvement needed to switch
max_grids = 1                 # simultaneous grids (symbols)

[autopilot.scanner]
min_turnover = 1000000.0      # minimum 24h turnover (USDT)
max_spread_pct = 0.2          # maximum bid-ask spread %
quote_currencies = ["USDT"]
max_results = 10              # top N candidates to analyze in depth
kline_interval = "60"         # candlestick interval (minutes)
kline_count = 200             # candles to fetch for analysis
```

## Running

```bash
# Via config
# Set autopilot.enabled = true in config.toml
./trading-bot config.toml

# Via CLI flag (overrides config)
./trading-bot config.toml --autopilot
```

## Multi-Grid

Set `max_grids > 1` to run grids on multiple symbols simultaneously. The bot will deploy grids on the top N ranked symbols and manage them independently.

> Multi-grid mode divides your equity across grids. Ensure your `starting_equity_quote` and risk limits account for this.

## Notes

- In auto-pilot mode, the `[grid]` config section (symbol, lower_price, upper_price) is ignored — these are computed dynamically
- Grid parameters (levels, qty) are still respected as base values for sizing
- All risk limits still apply — auto-pilot doesn't bypass the risk engine
- The scanner only uses public API endpoints (no credentials needed for scanning)
