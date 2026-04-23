# Systemd (VPS) Deployment

Run Zooni as a systemd service on any Linux VPS for production use.

## Setup

### 1. Create a Service User

```bash
sudo useradd -r -s /bin/false trading
```

### 2. Install the Binary

```bash
sudo mkdir -p /opt/trading-bot
sudo cp target/release/trading-bot /opt/trading-bot/
sudo cp config.toml /opt/trading-bot/
sudo chown -R trading:trading /opt/trading-bot
sudo chmod 600 /opt/trading-bot/config.toml
```

### 3. Install the Service File

```bash
sudo cp trading-bot.service /etc/systemd/system/
sudo systemctl daemon-reload
```

### 4. Start

```bash
sudo systemctl enable --now trading-bot
```

## Service File

The included `trading-bot.service` has security hardening:

```ini
[Unit]
Description=Trading Bot - Grid Strategy
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=trading
Group=trading
WorkingDirectory=/opt/trading-bot
ExecStart=/opt/trading-bot/trading-bot /opt/trading-bot/config.toml
Restart=always
RestartSec=10
Environment=RUST_LOG=info

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/trading-bot
PrivateTmp=true

# Resource limits
LimitNOFILE=65536
MemoryMax=512M

[Install]
WantedBy=multi-user.target
```

## Management

```bash
# Status
sudo systemctl status trading-bot

# Logs (live)
sudo journalctl -u trading-bot -f

# Logs (last hour)
sudo journalctl -u trading-bot --since "1 hour ago"

# Stop
sudo systemctl stop trading-bot

# Restart
sudo systemctl restart trading-bot

# Disable auto-start
sudo systemctl disable trading-bot
```

## Adding Telegram

Add environment variables to the service:

```bash
sudo systemctl edit trading-bot
```

```ini
[Service]
Environment=TELEGRAM_BOT_TOKEN=123456:ABC-DEF...
Environment=TELEGRAM_CHAT_ID=987654321
```

Then restart:

```bash
sudo systemctl restart trading-bot
```

## Updating

```bash
# Build new binary
cargo build --release

# Deploy
sudo systemctl stop trading-bot
sudo cp target/release/trading-bot /opt/trading-bot/
sudo systemctl start trading-bot
```

## Exposing the Dashboard

The dashboard listens on port 9090. To expose it:

```bash
# Simple: allow port through firewall
sudo ufw allow 9090

# Better: reverse proxy with nginx
# /etc/nginx/sites-available/zooni
server {
    listen 443 ssl;
    server_name zooni.yourdomain.com;

    location / {
        proxy_pass http://127.0.0.1:9090;
    }
}
```

> For a private dashboard, use SSH tunneling instead: `ssh -L 9090:localhost:9090 your-vps`
