# First Run

## 1. Start with Dry-Run on Testnet

Always test on testnet with dry-run first:

```bash
./target/release/zooni config.toml --dry-run
```

This simulates the full trading cycle without placing real orders. Watch the logs to verify:
- Grid levels are calculated correctly for your price range
- Risk checks pass for your equity and exposure settings
- Fill detection and order replacement logic works

## 2. Testnet with Real Orders

Once dry-run looks good, remove `--dry-run` but keep `testnet = true`:

```bash
./target/release/zooni config.toml
```

Log in to [Bybit Testnet](https://testnet.bybit.com/) to verify orders appear on the exchange.

## 3. Go Live

When you're confident:

1. Set `testnet = false` in `config.toml`
2. Replace API keys with your mainnet credentials
3. Set `starting_equity_quote` to your actual balance
4. Start conservatively — fewer levels, smaller `qty_per_level_base`

```bash
./target/release/zooni config.toml
```

## Running Modes

### Standard Mode

Trade a single symbol configured in `[grid]`:

```bash
./zooni config.toml
```

### TUI Mode

Same as standard, but with a live terminal dashboard:

```bash
./zooni config.toml --tui
```

### Auto-Pilot Mode

Fully autonomous — scans markets, picks symbols, sizes positions, and switches automatically:

```bash
# Enable in config
# [autopilot]
# enabled = true

./zooni config.toml --autopilot
```

See [Auto-Pilot Mode](../concepts/autopilot.md) for details.

## Graceful Shutdown

Press `Ctrl+C` to shut down. The bot will:

1. Stop placing new orders
2. Wait for in-flight API calls to complete (configurable via `shutdown_wait_secs`)
3. Cancel remaining open orders on the exchange
4. Save final state to the database

## Logs

Control verbosity with `RUST_LOG`:

```bash
# Default (info level)
RUST_LOG=info ./zooni config.toml

# Debug everything
RUST_LOG=debug ./zooni config.toml

# Debug bot logic, quiet HTTP noise
RUST_LOG=zooni=debug,reqwest=warn ./zooni config.toml
```
