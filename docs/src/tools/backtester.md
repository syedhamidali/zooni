# Backtester

The backtester simulates grid trading strategies against historical price data. Use it to find optimal parameters before risking real capital.

## Usage

```bash
# Backtest against a CSV price file
cargo run --release --bin backtest -- config.toml --data prices.csv

# Against synthetic generated data
cargo run --release --bin backtest -- config.toml --synthetic 5000

# Optimizer sweep — find the best levels/range combination
cargo run --release --bin backtest -- config.toml --optimize --data prices.csv

# Adaptive mode — ATR-based dynamic sizing
cargo run --release --bin backtest -- config.toml --adaptive --data prices.csv
```

## CLI Options

| Flag | Description |
|------|-------------|
| `--data <path>` | CSV file with historical prices |
| `--synthetic <N>` | Generate N bars of synthetic data instead |
| `--optimize` | Sweep over levels and price ranges, rank results |
| `--adaptive` | Use ATR-based adaptive position sizing |

## CSV Format

The price data CSV should have columns for OHLCV data. The backtester parses standard candlestick format.

## Output

### Standard Backtest

```
=== Backtest Results ===
Symbol: BTCUSDT
Period: 5000 bars
Levels: 20, Range: 60000.00 - 70000.00

Total PnL:     +245.30 USDT
Total Fills:   347
Win Rate:      68.4%
Sharpe Ratio:  1.82
Max Drawdown:  -3.2%
```

### Optimizer

The optimizer sweeps multiple configurations and ranks by composite score:

```
=== Optimization Results ===
Config       PnL      Sharpe  Drawdown  WinRate
15 levels  +312.40    2.10    -2.8%     72.1%
20 levels  +245.30    1.82    -3.2%     68.4%
25 levels  +198.70    1.65    -3.5%     65.2%

Best config: 15 levels, range 61000.00-69000.00
```

## Notes

- The backtester uses the same grid logic as the live bot — results are representative
- Synthetic data uses random walk with configurable volatility — useful for stress testing
- Fees are not simulated — subtract exchange fees from reported P&L for realistic estimates
- The backtester does not simulate order book depth or slippage
