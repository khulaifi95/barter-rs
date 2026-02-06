#!/usr/bin/env bash
# deploy/deploy.sh — Build and deploy barter-data-server to Hetzner VPS
#
# Prerequisites:
#   - musl-cross installed: brew install filosottile/musl-cross/musl-cross
#   - Rust target added: rustup target add x86_64-unknown-linux-musl
#   - SSH access to VPS: ssh deploy@$VPS_HOST
#
# Usage:
#   ./deploy/deploy.sh              # full deploy (build + transfer + restart)
#   ./deploy/deploy.sh --skip-build # transfer existing binary + restart
#   ./deploy/deploy.sh --build-only # just cross-compile, don't deploy

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DEPLOY_DIR="$SCRIPT_DIR"

# ── Configuration ──────────────────────────────────────────────
VPS_HOST="${VPS_HOST:-deploy@vps}"
VPS_DEPLOY_DIR="/home/deploy/barter"
MUSL_TARGET="x86_64-unknown-linux-musl"
BINARY_NAME="barter-data-server"
BINARY_SRC="$PROJECT_DIR/target/$MUSL_TARGET/release/$BINARY_NAME"

# ── Parse args ─────────────────────────────────────────────────
SKIP_BUILD=false
BUILD_ONLY=false
for arg in "$@"; do
    case "$arg" in
        --skip-build) SKIP_BUILD=true ;;
        --build-only) BUILD_ONLY=true ;;
        --help|-h)
            echo "Usage: $0 [--skip-build] [--build-only]"
            exit 0
            ;;
    esac
done

# ── Step 1: Cross-compile ─────────────────────────────────────
if [ "$SKIP_BUILD" = false ]; then
    echo "==> Cross-compiling $BINARY_NAME for $MUSL_TARGET..."
    cd "$PROJECT_DIR"
    cargo build -p barter-data-server --bin "$BINARY_NAME" \
        --target "$MUSL_TARGET" --release

    BINARY_SIZE=$(du -h "$BINARY_SRC" | cut -f1)
    echo "==> Binary built: $BINARY_SRC ($BINARY_SIZE)"

    # Verify it's a static binary
    file "$BINARY_SRC" | grep -q "static" && echo "==> Confirmed: static binary" \
        || echo "WARNING: binary may not be statically linked"

    # Strip debug symbols
    x86_64-linux-musl-strip "$BINARY_SRC" 2>/dev/null || true
    BINARY_SIZE=$(du -h "$BINARY_SRC" | cut -f1)
    echo "==> Stripped binary: $BINARY_SIZE"
fi

if [ "$BUILD_ONLY" = true ]; then
    echo "==> Build complete. Skipping deployment."
    exit 0
fi

# ── Step 2: Build container image ──────────────────────────────
echo "==> Copying binary to deploy/ context..."
cp "$BINARY_SRC" "$DEPLOY_DIR/$BINARY_NAME"

echo "==> Building barter-server container image (linux/amd64)..."
podman build --platform linux/amd64 \
    -f "$DEPLOY_DIR/Containerfile.barter-server" \
    -t barter-server:latest \
    "$DEPLOY_DIR"

echo "==> Saving container image..."
podman save barter-server:latest | gzip > "$DEPLOY_DIR/barter-server.tar.gz"
IMAGE_SIZE=$(du -h "$DEPLOY_DIR/barter-server.tar.gz" | cut -f1)
echo "==> Image saved: $DEPLOY_DIR/barter-server.tar.gz ($IMAGE_SIZE)"

# ── Step 3: Transfer to VPS ────────────────────────────────────
echo "==> Transferring files to $VPS_HOST:$VPS_DEPLOY_DIR..."
ssh "$VPS_HOST" "mkdir -p $VPS_DEPLOY_DIR"

scp "$DEPLOY_DIR/barter-server.tar.gz" "$VPS_HOST:$VPS_DEPLOY_DIR/"
scp "$DEPLOY_DIR/compose.yaml" "$VPS_HOST:$VPS_DEPLOY_DIR/"
scp "$DEPLOY_DIR/compose.prod.yaml" "$VPS_HOST:$VPS_DEPLOY_DIR/"
scp "$DEPLOY_DIR/.env.production" "$VPS_HOST:$VPS_DEPLOY_DIR/"
scp "$DEPLOY_DIR/status.sh" "$VPS_HOST:$VPS_DEPLOY_DIR/"
scp "$DEPLOY_DIR/backup.sh" "$VPS_HOST:$VPS_DEPLOY_DIR/"

# ── Step 4: Deploy on VPS ──────────────────────────────────────
echo "==> Deploying on VPS..."
ssh "$VPS_HOST" bash <<'REMOTE'
set -euo pipefail
cd ~/barter

# Copy env if not exists (don't overwrite running config)
if [ ! -f .env ]; then
    cp .env.production .env
    echo "  Created .env from .env.production"
fi

# Load new image
echo "  Loading container image..."
podman load < barter-server.tar.gz
rm barter-server.tar.gz

# Restart barter-server only (preserve nautilus if running)
echo "  Restarting barter-server..."
podman-compose -f compose.yaml -f compose.prod.yaml stop barter-server 2>/dev/null || true
podman-compose -f compose.yaml -f compose.prod.yaml up -d barter-server

# Wait for healthcheck
echo "  Waiting for health check (up to 120s)..."
for i in $(seq 1 12); do
    sleep 10
    if [ -f /data/ipc/collector-heartbeat.json ]; then
        echo "  Heartbeat file found after ${i}0s"
        cat /data/ipc/collector-heartbeat.json | python3 -m json.tool 2>/dev/null || cat /data/ipc/collector-heartbeat.json
        break
    fi
    echo "  Waiting... (${i}0s)"
done

echo "  Container status:"
podman ps --format "table {{.Names}}\t{{.Status}}\t{{.Ports}}"
REMOTE

echo ""
echo "==> Deploy complete!"
echo "    SSH:    ssh $VPS_HOST"
echo "    Logs:   ssh $VPS_HOST 'podman logs -f barter-server'"
echo "    Status: ssh $VPS_HOST 'bash ~/barter/status.sh'"

# Cleanup local artifacts
rm -f "$DEPLOY_DIR/$BINARY_NAME"
rm -f "$DEPLOY_DIR/barter-server.tar.gz"
