# CI/CD

Zooni uses GitHub Actions for continuous integration and automated releases.

## Pipeline

Every push and PR to `main` triggers:

```
┌─────────────┐     ┌─────────┐     ┌─────────────┐     ┌─────────────┐
│ Check & Lint│ ──> │  Test   │ ──> │ Build Release│ ──> │ Auto-Release│
│ fmt, clippy │     │ all tests│    │  binaries    │     │ (main only) │
└─────────────┘     └─────────┘     └─────────────┘     └─────────────┘
```

### 1. Check & Lint

- `cargo fmt --all -- --check` — enforces consistent formatting
- `cargo clippy --all-targets --all-features` — catches common mistakes, treated as errors (`-D warnings`)

### 2. Test

- `cargo test --all` — runs all 64 tests across all crates

### 3. Build Release

- `cargo build --release --all-targets` — full release build
- Uploads binaries (`zooni`, `backtest`, `scan`) as GitHub Actions artifacts

### 4. Auto-Release

On pushes to `main` only:

- Generates a date-based version tag: `v2026.4.23.<run_number>`
- Creates a GitHub Release with the binaries attached
- Includes auto-generated release notes from commit messages

## Versioning

Tags follow the pattern `vYYYY.M.D.N` where N is the GitHub Actions run number. This handles multiple releases per day.

## Workflow File

The pipeline is defined in `.github/workflows/ci.yml`. Key settings:

- `RUSTFLAGS: "-D warnings"` — all warnings are errors
- Uses `dtolnay/rust-toolchain@stable` for consistent Rust versions
- Uses `Swatinem/rust-cache@v2` to cache cargo builds between runs
- Release job requires `contents: write` permission

## Running Locally

Replicate what CI does:

```bash
# Same checks CI runs
cargo fmt --all -- --check
cargo clippy --all-targets --all-features
cargo test --all
cargo build --release --all-targets
```
