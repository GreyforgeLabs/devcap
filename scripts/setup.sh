#!/usr/bin/env bash
set -euo pipefail

# devcap setup script
# Idempotent - safe to run multiple times

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

echo "=== devcap Setup ==="

check_command() {
    if ! command -v "$1" &> /dev/null; then
        echo "ERROR: $1 is required but not installed. Install Rust from https://rustup.rs"
        exit 1
    fi
}

check_command cargo

cd "$PROJECT_DIR"

cargo build --release --locked

echo "=== Setup complete ==="
echo "Binary: $PROJECT_DIR/target/release/devcap"
echo "Install onto PATH with: cargo install --path . --locked"

echo "Running verification..."
"$PROJECT_DIR/target/release/devcap" list-profiles
"$PROJECT_DIR/target/release/devcap" scan --profile python-dev --format json > /dev/null
echo "Verification scan completed."
