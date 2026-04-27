# Docker Deployment

Zooni ships with a multi-stage Dockerfile that produces a minimal runtime image.

## Build

```bash
docker build -t zooni .
```

The build:
1. Compiles all three binaries (`zooni`, `backtest`, `scan`) in a Rust builder stage
2. Copies only the binaries into a slim Debian runtime image
3. Final image includes just `ca-certificates` for HTTPS

## Run

```bash
# Basic run
docker run -v $(pwd)/config.toml:/data/config.toml zooni

# With Telegram
docker run \
  -e TELEGRAM_BOT_TOKEN="123456:ABC..." \
  -e TELEGRAM_CHAT_ID="987654321" \
  -v $(pwd)/config.toml:/data/config.toml \
  zooni

# Expose dashboard
docker run \
  -p 9090:9090 \
  -v $(pwd)/config.toml:/data/config.toml \
  zooni

# Persistent database
docker run \
  -v $(pwd)/config.toml:/data/config.toml \
  -v $(pwd)/data:/data \
  zooni

# Dry-run mode
docker run \
  -v $(pwd)/config.toml:/data/config.toml \
  zooni config.toml --dry-run
```

## Docker Compose

```yaml
services:
  zooni:
    build: .
    restart: unless-stopped
    ports:
      - "9090:9090"
    volumes:
      - ./config.toml:/data/config.toml:ro
      - zooni-data:/data
    environment:
      - RUST_LOG=info
      - TELEGRAM_BOT_TOKEN=${TELEGRAM_BOT_TOKEN}
      - TELEGRAM_CHAT_ID=${TELEGRAM_CHAT_ID}

volumes:
  zooni-data:
```

## Scanner in Docker

```bash
docker run --rm \
  -v $(pwd)/config.toml:/data/config.toml \
  --entrypoint scan \
  zooni --config config.toml --top 10
```

## Notes

- Config is expected at `/data/config.toml`
- Database is created at `/data/zooni.db`
- Mount `/data` as a volume to persist the database across restarts
- The image is based on `debian:bookworm-slim` (~80MB)
