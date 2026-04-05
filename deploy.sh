#!/usr/bin/env bash
set -euo pipefail

# Deploy lific to the basement server.
# Builds a release binary, copies it over, and restarts the service.
#
# Usage:
#   ./deploy.sh          # build + deploy
#   ./deploy.sh --skip   # deploy existing binary without rebuilding

REMOTE="blake@basement"
REMOTE_DIR="/opt/ada/lific"
BINARY="target/release/lific"

if [[ "${1:-}" != "--skip" ]]; then
    echo ":: building frontend..."
    (cd web && bun install --frozen-lockfile && bun run build)

    echo ":: building release binary..."
    cargo build --release
fi

if [[ ! -f "$BINARY" ]]; then
    echo "error: no binary at $BINARY — run without --skip first"
    exit 1
fi

echo ":: copying binary to $REMOTE:$REMOTE_DIR/lific"
scp "$BINARY" "$REMOTE:$REMOTE_DIR/lific.new"

echo ":: swapping binary and restarting service..."
ssh "$REMOTE" "
    mv $REMOTE_DIR/lific.new $REMOTE_DIR/lific &&
    chmod +x $REMOTE_DIR/lific &&
    echo 'Bw171717!' | sudo -S systemctl restart lific
"

echo ":: verifying..."
sleep 2
ssh "$REMOTE" "systemctl is-active lific && $REMOTE_DIR/lific --version"

echo ":: deployed successfully"
