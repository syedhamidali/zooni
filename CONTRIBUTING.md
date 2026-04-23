# Contributing to Zooni

Thank you for your interest in contributing to Zooni.

## Getting Started

1. Fork the repository
2. Clone your fork: `git clone https://github.com/<you>/zooni.git`
3. Create a branch: `git checkout -b my-feature`
4. Make your changes
5. Run checks: `cargo fmt && cargo clippy && cargo test`
6. Commit and push
7. Open a pull request

## Development Setup

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Build
cargo build

# Run tests
cargo test --all

# Run lints
cargo clippy --all-targets --all-features

# Format
cargo fmt --all
```

## Code Standards

- All code must pass `cargo clippy` with no warnings (`-D warnings`)
- All code must be formatted with `cargo fmt`
- New features should include tests
- Commit messages should be concise and descriptive
- PRs should target the `main` branch

## Architecture

See the [Architecture docs](docs/src/reference/architecture.md) for an overview of how the codebase is structured.

## Reporting Issues

- Use the [bug report template](.github/ISSUE_TEMPLATE/bug_report.md) for bugs
- Use the [feature request template](.github/ISSUE_TEMPLATE/feature_request.md) for new ideas
- Check existing issues before opening a new one

## Security

If you discover a security vulnerability, please report it privately. See [SECURITY.md](SECURITY.md).
