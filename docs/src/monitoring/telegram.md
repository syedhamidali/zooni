# Telegram Bot

Zooni includes a Telegram bot for remote monitoring and control. It polls for commands and responds with live bot status.

## Setup

### 1. Create a Telegram Bot

1. Message [@BotFather](https://t.me/BotFather) on Telegram
2. Send `/newbot` and follow the prompts
3. Copy the bot token (e.g., `123456:ABC-DEF...`)

### 2. Get Your Chat ID

1. Message your new bot
2. Visit `https://api.telegram.org/bot<TOKEN>/getUpdates`
3. Find your `chat.id` in the response

### 3. Set Environment Variables

```bash
export TELEGRAM_BOT_TOKEN="123456:ABC-DEF..."
export TELEGRAM_CHAT_ID="987654321"
```

The bot starts automatically when both variables are set.

## Commands

| Command | Description |
|---------|-------------|
| `/status` | Current bot status (mode, uptime, circuit breaker) |
| `/pnl` | Daily and total P&L |
| `/grid` | Active grid levels and fill count |
| `/pause` | Pause trading (keeps bot running) |
| `/resume` | Resume trading after pause |
| `/fills` | Recent fill history |

## How It Works

- The Telegram module polls `getUpdates` every 5 seconds
- Only messages from the configured `TELEGRAM_CHAT_ID` are processed
- Status data comes from shared `Arc<Metrics>` — same data as the web dashboard
- The bot runs as a background task and doesn't interfere with trading

## Security

- Only your configured chat ID can send commands
- The bot token should be kept secret — never commit it
- Consider using a dedicated bot for each bot instance
