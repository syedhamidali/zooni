# Systemd (VPS) Deployment

Run Zooni as a systemd service on any Linux VPS for production use.

## Setup

### 1. Create a Service User

```bash
sudo useradd -r -s /bin/false trading
```

### 2. Install the Binary

```bash
sudo mkdir -p /opt/zooni
sudo cp target/release/zooni /opt/zooni/
sudo cp config.toml /opt/zooni/
sudo chown -R zooni:zooni /opt/zooni
sudo chmod 600 /opt/zooni/config.toml
```

### 3. Install the Service File

```bash
sudo cp zooni.service /etc/systemd/system/
sudo systemctl daemon-reload
```

### 4. Start

```bash
sudo systemctl enable --now zooni
```

## Service File

The included `zooni.service` has security hardening:

```ini
[Unit]
Description=Trading Bot - Grid Strategy
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=zooni
Group=zooni
WorkingDirectory=/opt/zooni
ExecStart=/opt/zooni/zooni /opt/zooni/config.toml
Restart=always
RestartSec=10
Environment=RUST_LOG=info

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/zooni
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
sudo systemctl status zooni

# Logs (live)
sudo journalctl -u zooni -f

# Logs (last hour)
sudo journalctl -u zooni --since "1 hour ago"

# Stop
sudo systemctl stop zooni

# Restart
sudo systemctl restart zooni

# Disable auto-start
sudo systemctl disable zooni
```

## Adding Telegram

Add environment variables to the service:

```bash
sudo systemctl edit zooni
```

```ini
[Service]
Environment=TELEGRAM_BOT_TOKEN=123456:ABC-DEF...
Environment=TELEGRAM_CHAT_ID=987654321
```

Then restart:

```bash
sudo systemctl restart zooni
```

## Updating

```bash
# Build new binary
cargo build --release

# Deploy
sudo systemctl stop zooni
sudo cp target/release/zooni /opt/zooni/
sudo systemctl start zooni
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
