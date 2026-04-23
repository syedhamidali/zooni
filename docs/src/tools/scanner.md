# Scanner

The scanner is a standalone tool that analyzes all available trading pairs and ranks them by grid-trading suitability. It's the same engine used by auto-pilot mode, but runs as a one-shot CLI.

## Usage

```bash
# Basic scan with defaults
cargo run --release --bin scan -- --config config.toml

# Top 5 results
cargo run --release --bin scan -- --config config.toml --top 5

# Custom filters
cargo run --release --bin scan -- \
  --config config.toml \
  --quote USDT \
  --min-turnover 5000000 \
  --top 10
```

## CLI Options

| Flag | Default | Description |
|------|---------|-------------|
| `--config` | `config.toml` | Config file path (uses `[bybit]` for API endpoint) |
| `--top` | `10` | Number of top results to show |
| `--quote` | `USDT` | Quote currency to filter by |
| `--min-turnover` | From config | Minimum 24h turnover in quote currency |
| `--interval` | `60` | Kline interval in minutes |

## Output

The scanner prints a formatted report:

```
=== Market Scan Report ===
Scanned 150 pairs, analyzed top 10

#1  ETHUSDT    score: 82.3  regime: Ranging
    ADX: 18.2  RSI: 48.7  BB width: 3.2%  Vol: 1.8%
    Range: 3,200.00 - 3,450.00  Levels: 15

#2  SOLUSDT    score: 71.5  regime: Ranging
    ADX: 21.4  RSI: 52.1  BB width: 4.1%  Vol: 2.3%
    Range: 145.00 - 158.00  Levels: 12
...
```

Followed by a CSV summary for easy import into spreadsheets.

## What Gets Scored

1. **Pre-filter**: Remove pairs with low turnover or wide spreads
2. **Fetch klines**: Get recent candlestick data for each candidate
3. **Regime analysis**: Compute ADX, RSI, Bollinger Bands, ATR
4. **Score**: Composite of regime suitability, volatility, and liquidity
5. **Rank**: Sort by score, highest first

## Scanner Configuration

Fine-tune via `[autopilot.scanner]` in your config:

```toml
[autopilot.scanner]
min_turnover = 1000000.0    # filter out illiquid pairs
max_spread_pct = 0.2        # filter out wide-spread pairs
quote_currencies = ["USDT"] # which quote currencies
max_results = 10            # analyze top N after pre-filter
kline_interval = "60"       # candle interval (minutes)
kline_count = 200           # how many candles to fetch
```

## Notes

- The scanner uses **public API endpoints only** — no API credentials required for scanning
- Kline fetches are rate-limited to avoid hitting exchange limits
- Scan time depends on `max_results` (each candidate requires a kline fetch)
