# Grid Trading

Grid trading is an automated strategy that profits from price oscillations within a range. It works best in **sideways/ranging markets** where price bounces between support and resistance.

## How It Works

1. The bot divides a price range into evenly spaced **grid levels**
2. **Buy orders** are placed below the current price
3. **Sell orders** are placed above the current price
4. When a **buy fills**, a corresponding sell is placed one level up
5. When a **sell fills**, a corresponding buy is placed one level down
6. Each complete buy-sell cycle captures the **grid spacing** as profit

```
Price
  ^
  |  -------- SELL @ 65,000  (placed after buy fills)
  |  -------- SELL @ 64,000
  |  ~~~~~~~~ current price ~63,500
  |  -------- BUY  @ 63,000
  |  -------- BUY  @ 62,000  (placed after sell fills)
  |
```

## Profit Per Cycle

Each complete buy-sell round trip earns approximately:

```
profit = grid_spacing * qty_per_level
```

For example, with 20 levels across 60,000–70,000 USDT and 0.001 BTC per level:
- Grid spacing = (70,000 - 60,000) / 20 = 500 USDT
- Profit per cycle = 500 * 0.001 = 0.50 USDT (minus fees)

## Configuration

```toml
[grid]
symbol = "BTCUSDT"
lower_price = 60000.0    # grid bottom
upper_price = 70000.0    # grid top
levels = 20              # number of grid lines
qty_per_level_base = 0.001  # BTC per order
poll_secs = 3            # check for fills every 3 seconds
```

## Key Parameters

### Levels

More levels = tighter spacing = more frequent trades = smaller profit per trade. Fewer levels = wider spacing = less frequent but larger profits. Find the sweet spot for your market's typical oscillation range.

### Price Range

- **Too narrow**: Price breaks out quickly, grid becomes one-sided
- **Too wide**: Spacing is too large, trades are infrequent
- **Right size**: Covers the recent support/resistance zone

### Auto-Rebalance

When price exits the grid range, you can either:

- **Wait** for price to return (default behavior)
- **Auto-rebalance** — shift the entire grid to re-center around current price

```toml
auto_rebalance = true
```

### Trailing Profit

Lock in accumulated gains when grid fills exceed a threshold:

```toml
trailing_profit_threshold = 50.0  # USDT — take profit above this
```

## When Grid Trading Works

| Market Condition | Grid Performance |
|-----------------|-----------------|
| Ranging/sideways | Excellent — frequent fills, consistent profit |
| Low volatility range | Good — steady but slower |
| Trending up | Poor — all buys fill, sells don't |
| Trending down | Poor — all sells fill, buys don't |
| High volatility | Risky — potential for large unrealized losses |

Zooni's [market intelligence](./market-intelligence.md) detects these regimes and can pause or switch symbols automatically in [auto-pilot mode](./autopilot.md).

## Fill Detection

The bot polls open orders every `poll_secs` seconds. When an order disappears from the open orders list, it's treated as filled, and a replacement order is placed on the opposite side.

## Order Deduplication

Every order gets a unique `orderLinkId` to prevent duplicate placements during retries or restarts.
