# Multi-stage build for minimal image size
FROM rust:1.82-slim AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src/ ./src/

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
RUN cargo build --release --bin trading-bot --bin backtest --bin scan

# Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/trading-bot /usr/local/bin/trading-bot
COPY --from=builder /app/target/release/backtest /usr/local/bin/backtest
COPY --from=builder /app/target/release/scan /usr/local/bin/scan

WORKDIR /data
VOLUME ["/data"]

# Config is expected at /data/config.toml
# Database will be created at /data/trading-bot.db

ENTRYPOINT ["trading-bot"]
CMD ["config.toml"]
