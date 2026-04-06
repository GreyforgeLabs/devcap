#!/usr/bin/env bash
set -euo pipefail

# devcap setup script
# Idempotent - safe to run multiple times

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

echo "=== devcap Setup ==="

check_command() {
    if ! command -v "$1" &> /dev/null; then
        echo "ERROR: $1 is required but not installed."
        exit 1
    fi
}

check_command python3

PYTHON_VERSION="$(python3 - <<'PY'
import sys
print(f"{sys.version_info.major}.{sys.version_info.minor}")
PY
)"

case "$PYTHON_VERSION" in
    3.11|3.12|3.13) ;;
    *)
        echo "ERROR: Python 3.11+ is required. Found $PYTHON_VERSION."
        exit 1
        ;;
esac

cd "$PROJECT_DIR"

if [ ! -d ".venv" ]; then
    python3 -m venv .venv
fi

# shellcheck disable=SC1091
source .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -e ".[dev]"

echo "=== Setup complete ==="

echo "Running verification..."
devcap scan --profile python-dev --format json | python3 -c "import sys, json; d=json.load(sys.stdin); print(f'Scanned {len(d[\"tools\"])} tools on {d[\"hostname\"]}')"
