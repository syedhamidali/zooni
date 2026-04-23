# Supported Venues

## Bybit (Active)

Full implementation for Bybit spot markets via the V5 API.

### Capabilities

| Feature | Status |
|---------|--------|
| Spot limit orders | Implemented |
| Market orders | Implemented |
| Cancel orders | Implemented |
| Open order polling | Implemented |
| Wallet balance | Implemented |
| All tickers (24h) | Implemented |
| Klines (OHLCV) | Implemented |
| Server time sync | Implemented |
| HMAC-SHA256 signing | Implemented |
| WebSocket (private) | Implemented |
| Testnet support | Implemented |

### Endpoints

| Environment | Base URL |
|-------------|----------|
| Mainnet | `https://api.bybit.com` |
| Testnet | `https://api-testnet.bybit.com` |

### API Version

Uses V5 unified API. All endpoints go through `/v5/`.

---

## Hyperliquid (Planned)

Skeleton implementation exists. REST and WebSocket stubs are in place.

### Target Features
- Perpetual futures trading
- On-chain order book
- No KYC required

---

## Zerodha (Planned)

Skeleton implementation for Indian equities via Kite Connect v3.

### Target Features
- NSE/BSE equity trading
- F&O (futures and options)
- MCX commodities (via Zerodha's MCX membership)
- Market hours awareness for Indian sessions

### Notes
- MCX (Multi Commodity Exchange) is accessed through Indian brokers like Zerodha, not as a standalone API
- Requires Zerodha Kite Connect subscription

---

## Interactive Brokers (Planned)

Skeleton implementation for US equities via TWS/IB Gateway.

### Target Features
- US stock trading
- Options
- TWS API / IB Gateway connection

### Notes
- Requires Interactive Brokers account
- Connects via TWS (Trader Workstation) or IB Gateway
- Different API model from REST-based exchanges

---

## Adding a New Venue

The venue abstraction lives in `src/venues/`. To add a new exchange:

1. Create a new directory under `src/venues/` (e.g., `src/venues/binance/`)
2. Implement the REST client with methods matching the Bybit interface:
   - `place_order()`, `cancel_order()`, `open_orders()`
   - `wallet_balance()`, `server_time()`
   - `all_tickers_spot()`, `klines()` (for scanner support)
3. Add request signing if required
4. Register the module in `src/venues/mod.rs`
5. Wire it into `bot.rs` with a venue selection mechanism
