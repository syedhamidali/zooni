#!/usr/bin/env bash
# deploy.sh — build on VPS and restart zooni
#
# Usage: ./scripts/deploy.sh
#
# What it does:
#   1. git pull on the VPS
#   2. cargo build --release on the VPS
#   3. sudo cp binary → /opt/zooni/zooni   (NEVER touches /opt/zooni/config.toml)
#   4. sudo systemctl restart zooni
#
# /opt/zooni/config.toml is the permanent production config.
# It contains secrets (API keys, Telegram token) and is NEVER overwritten by this script.
# To update the config, edit /opt/zooni/config.toml directly on the VPS.

set -euo pipefail

VPS_USER="ubuntu"
VPS_HOST="51.79.234.66"
VPS_PORT="22"
REMOTE_SRC="$HOME/zooni"
REMOTE_BIN="/opt/zooni/zooni"
SERVICE="zooni"

echo "==> Deploying to $VPS_HOST..."

ssh -p "$VPS_PORT" "$VPS_USER@$VPS_HOST" bash << 'REMOTE'
set -euo pipefail

echo "--- Pulling latest code ---"
cd ~/zooni
git pull

echo "--- Building release binary ---"
cargo build --release 2>&1

echo "--- Installing binary (config untouched) ---"
sudo cp target/release/zooni /opt/zooni/zooni
sudo chown zooni:zooni /opt/zooni/zooni
sudo chmod 755 /opt/zooni/zooni

echo "--- Restarting service ---"
sudo systemctl restart zooni
sleep 2
systemctl status zooni --no-pager | head -15

echo "--- Done ---"
REMOTE

echo "==> Deploy complete."
