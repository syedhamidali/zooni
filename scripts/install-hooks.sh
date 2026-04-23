#!/usr/bin/env bash
# Install git hooks for the zooni project.
# Run once after cloning: ./scripts/install-hooks.sh

set -euo pipefail

HOOK_DIR="$(git rev-parse --git-dir)/hooks"
mkdir -p "$HOOK_DIR"

cat > "$HOOK_DIR/pre-commit" << 'HOOK'
#!/usr/bin/env bash
# Auto-format staged Rust files before commit.
# If cargo fmt changes anything, the fixed files are re-staged automatically.

set -euo pipefail

if ! command -v cargo &>/dev/null; then
    echo "warning: cargo not found, skipping format check"
    exit 0
fi

# Run cargo fmt on the whole project
cargo fmt --all -- --check 2>/dev/null
FMT_EXIT=$?

if [ $FMT_EXIT -ne 0 ]; then
    echo "Running cargo fmt to fix formatting..."
    cargo fmt --all
    # Re-stage the formatted files
    git diff --name-only --cached | grep '\.rs$' | xargs -r git add
    echo "Formatted and re-staged. Proceeding with commit."
fi
HOOK

chmod +x "$HOOK_DIR/pre-commit"
echo "Installed pre-commit hook (auto-formats Rust code)."
