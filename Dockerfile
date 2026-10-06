# Multi-stage build for minimal image size
FROM rust:1.99-slim-bookworm AS builder

WORKDIR /app
COPY Cargo.toml Cargo.lock* ./
COPY src/ ./src/
COPY build.rs ./
# The [[bench]] target declared in Cargo.toml must exist or cargo refuses to parse the manifest
COPY benches/ ./benches/

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
RUN cargo build --release --bin zooni --bin zooni-backtest --bin zooni-scan

# Runtime image
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/zooni /usr/local/bin/zooni
COPY --from=builder /app/target/release/zooni-backtest /usr/local/bin/zooni-backtest
COPY --from=builder /app/target/release/zooni-scan /usr/local/bin/zooni-scan

WORKDIR /data
VOLUME ["/data"]

# Config is expected at /data/config.toml
# Database will be created at /data/zooni.db

ENTRYPOINT ["zooni"]
CMD ["config.toml"]
