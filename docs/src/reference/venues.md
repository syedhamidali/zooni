# Supported Venues

## Bybit (Active)

Full implementation for Bybit spot and perpetual futures markets via the V5 API.

### Spot Capabilities

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

### Linear / Perpetual Capabilities

| Feature | Status |
|---------|--------|
| Linear tickers (funding rate, mark price, OI) | Implemented |
| Funding rate history | Implemented |
| Place linear orders (with `reduce_only`) | Implemented |
| Cancel linear orders | Implemented |
| Cancel all linear orders | Implemented |
| Open orders (linear) | Implemented |
| Position queries (unrealised P&L) | Implemented |
| Set leverage | Implemented |
| Instrument info (linear) | Implemented |

### Endpoints

| Environment | Base URL |
|-------------|----------|
| Mainnet | `https://api.bybit.com` |
| Testnet | `https://api-testnet.bybit.com` |

### API Version

Uses V5 unified API. All endpoints go through `/v5/`.

---

## Zerodha (Active)

Full implementation for Indian equities, F&O, and commodities via Kite Connect v3.

### Capabilities

| Feature | Status |
|---------|--------|
| Place orders (regular, AMO, CO, iceberg) | Implemented |
| Modify / cancel orders | Implemented |
| Order book & trade book | Implemented |
| Positions & holdings | Implemented |
| Margins (equity + commodity) | Implemented |
| User profile | Implemented |
| Instrument master (CSV) | Implemented |
| Market quotes (LTP, OHLC, full) | Implemented |
| Session token exchange (SHA256 checksum) | Implemented |
| Binary WebSocket ticker (LTP, Quote, Full + L5 depth) | Implemented |
| Auto-reconnect WebSocket | Implemented |

### Supported Exchanges

| Exchange | Segment |
|----------|---------|
| NSE | Equity, F&O |
| BSE | Equity, F&O |
| MCX | Commodities |
| CDS | Currency derivatives |
| BFO | BSE F&O |

### Authentication

Zerodha uses a daily access token flow:

1. Redirect user to `login_url()` (Kite login page)
2. Receive `request_token` via callback
3. Exchange for `access_token` using SHA256 checksum of `api_key + request_token + api_secret`
4. Token is valid for one trading day

### Notes
- Access token must be refreshed daily (before market open)
- WebSocket uses a binary protocol — prices are in paise (÷ 100 for rupees)
- Full tick mode provides Level 5 market depth (5 best bid/ask levels)
- Requires a Kite Connect subscription from Zerodha

---

## Hyperliquid (Planned)

Skeleton implementation exists. REST and WebSocket stubs are in place.

### Target Features
- Perpetual futures trading
- On-chain order book
- No KYC required

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
