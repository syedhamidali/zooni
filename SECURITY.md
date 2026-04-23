# Security Policy

## Reporting a Vulnerability

If you discover a security vulnerability in Zooni, please report it responsibly.

**Do not open a public issue.** Instead, email [hamidrixvi@gmail.com](mailto:hamidrixvi@gmail.com) with:

- Description of the vulnerability
- Steps to reproduce
- Potential impact

You will receive a response within 48 hours.

## Scope

Security issues include but are not limited to:

- API key exposure or leakage
- Authentication bypass
- Injection vulnerabilities in config parsing
- Unauthorized order placement
- Data exfiltration from the database

## Best Practices for Users

- Never commit `config.toml` (it's in `.gitignore`)
- Set file permissions: `chmod 600 config.toml`
- Use testnet for initial setup
- Run with `dry_run = true` until you've verified behavior
- Use a dedicated API key with minimal permissions
- Run the bot as a non-root user (see systemd service)
