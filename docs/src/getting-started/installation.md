# Installation

## Prerequisites

- [Rust](https://rustup.rs/) toolchain (edition 2024)
- A Bybit account — [testnet](https://testnet.bybit.com/) recommended for initial setup

## Build from Source

```bash
git clone https://github.com/syedhamidali/zooni.git
cd zooni
cargo build --release
```

This produces three binaries in `target/release/`:

| Binary | Description |
|--------|-------------|
| `zooni` | Main trading bot |
| `backtest` | Strategy backtester |
| `scan` | Market scanner |

## Docker

```bash
docker build -t zooni .
```

See [Docker deployment](../deployment/docker.md) for details.

## GitHub Releases

Pre-built Linux binaries are attached to every [GitHub release](https://github.com/syedhamidali/zooni/releases). Download and run directly — no Rust toolchain needed.

## Verify Installation

```bash
./target/release/zooni --version
# zooni 2026.4.23 (abc1234)
```
