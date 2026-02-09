#!/usr/bin/env bash
# deploy/deploy.sh — Build and deploy barter-data-server + barter-features to Hetzner VPS
#
# Prerequisites:
#   - musl-cross installed: brew install filosottile/musl-cross/musl-cross
#   - Rust target added: rustup target add x86_64-unknown-linux-musl
#   - SSH access to VPS: ssh deployer@$VPS_HOST
#
# Usage:
#   ./deploy/deploy.sh              # full deploy (build + transfer + restart)
#   ./deploy/deploy.sh --skip-build # transfer existing binaries + restart
#   ./deploy/deploy.sh --build-only # just cross-compile, don't deploy
#   ./deploy/deploy.sh --server-only # only deploy barter-server
#   ./deploy/deploy.sh --features-only # only deploy barter-features
#   ./deploy/deploy.sh --jupyter-only # only build+deploy jupyter image (on VPS)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
DEPLOY_DIR="$SCRIPT_DIR"

# ── Configuration ──────────────────────────────────────────────
VPS_HOST="${VPS_HOST:-deployer@vps}"
VPS_DEPLOY_DIR="/home/deployer/barter"
MUSL_TARGET="x86_64-unknown-linux-musl"

# Binary names
SERVER_BINARY="barter-data-server"
FEATURES_BINARY="barter-features"
SERVER_SRC="$PROJECT_DIR/target/$MUSL_TARGET/release/$SERVER_BINARY"
FEATURES_SRC="$PROJECT_DIR/target/$MUSL_TARGET/release/$FEATURES_BINARY"

# ── Parse args ─────────────────────────────────────────────────
SKIP_BUILD=false
BUILD_ONLY=false
SERVER_ONLY=false
FEATURES_ONLY=false
JUPYTER_ONLY=false
for arg in "$@"; do
    case "$arg" in
        --skip-build) SKIP_BUILD=true ;;
        --build-only) BUILD_ONLY=true ;;
        --server-only) SERVER_ONLY=true ;;
        --features-only) FEATURES_ONLY=true ;;
        --jupyter-only) JUPYTER_ONLY=true ;;
        --help|-h)
            echo "Usage: $0 [--skip-build] [--build-only] [--server-only] [--features-only] [--jupyter-only]"
            exit 0
            ;;
    esac
done

# Determine which components to deploy
DEPLOY_SERVER=true
DEPLOY_FEATURES=true
DEPLOY_JUPYTER=false
if [ "$SERVER_ONLY" = true ]; then
    DEPLOY_FEATURES=false
fi
if [ "$FEATURES_ONLY" = true ]; then
    DEPLOY_SERVER=false
fi
if [ "$JUPYTER_ONLY" = true ]; then
    DEPLOY_SERVER=false
    DEPLOY_FEATURES=false
    DEPLOY_JUPYTER=true
    SKIP_BUILD=true   # no Rust cross-compile needed
fi

# ── Step 1: Cross-compile ─────────────────────────────────────
if [ "$SKIP_BUILD" = false ]; then
    cd "$PROJECT_DIR"

    if [ "$DEPLOY_SERVER" = true ]; then
        echo "==> Cross-compiling $SERVER_BINARY for $MUSL_TARGET..."
        cargo build -p barter-data-server --bin "$SERVER_BINARY" \
            --target "$MUSL_TARGET" --release
        BINARY_SIZE=$(du -h "$SERVER_SRC" | cut -f1)
        echo "==> Binary built: $SERVER_SRC ($BINARY_SIZE)"
        file "$SERVER_SRC" | grep -q "static" && echo "==> Confirmed: static binary" \
            || echo "WARNING: binary may not be statically linked"
        x86_64-linux-musl-strip "$SERVER_SRC" 2>/dev/null || true
        BINARY_SIZE=$(du -h "$SERVER_SRC" | cut -f1)
        echo "==> Stripped binary: $BINARY_SIZE"
    fi

    if [ "$DEPLOY_FEATURES" = true ]; then
        echo "==> Cross-compiling $FEATURES_BINARY for $MUSL_TARGET..."
        cargo build -p barter-features --bin "$FEATURES_BINARY" \
            --target "$MUSL_TARGET" --release
        BINARY_SIZE=$(du -h "$FEATURES_SRC" | cut -f1)
        echo "==> Binary built: $FEATURES_SRC ($BINARY_SIZE)"
        file "$FEATURES_SRC" | grep -q "static" && echo "==> Confirmed: static binary" \
            || echo "WARNING: binary may not be statically linked"
        x86_64-linux-musl-strip "$FEATURES_SRC" 2>/dev/null || true
        BINARY_SIZE=$(du -h "$FEATURES_SRC" | cut -f1)
        echo "==> Stripped binary: $BINARY_SIZE"
    fi
fi

if [ "$BUILD_ONLY" = true ]; then
    echo "==> Build complete. Skipping deployment."
    exit 0
fi

# ── Step 2: Build container images ────────────────────────────
if [ "$DEPLOY_SERVER" = true ]; then
    echo "==> Copying $SERVER_BINARY to deploy/ context..."
    cp "$SERVER_SRC" "$DEPLOY_DIR/$SERVER_BINARY"
    echo "==> Building barter-server container image (linux/amd64)..."
    podman build --platform linux/amd64 \
        -f "$DEPLOY_DIR/Containerfile.barter-server" \
        -t barter-server:latest \
        "$DEPLOY_DIR"
    echo "==> Saving barter-server image..."
    podman save barter-server:latest | gzip > "$DEPLOY_DIR/barter-server.tar.gz"
    IMAGE_SIZE=$(du -h "$DEPLOY_DIR/barter-server.tar.gz" | cut -f1)
    echo "==> Image saved: $DEPLOY_DIR/barter-server.tar.gz ($IMAGE_SIZE)"
fi

if [ "$DEPLOY_FEATURES" = true ]; then
    echo "==> Copying $FEATURES_BINARY to deploy/ context..."
    cp "$FEATURES_SRC" "$DEPLOY_DIR/$FEATURES_BINARY"
    echo "==> Building barter-features container image (linux/amd64)..."
    podman build --platform linux/amd64 \
        -f "$DEPLOY_DIR/Containerfile.barter-features" \
        -t barter-features:latest \
        "$DEPLOY_DIR"
    echo "==> Saving barter-features image..."
    podman save barter-features:latest | gzip > "$DEPLOY_DIR/barter-features.tar.gz"
    IMAGE_SIZE=$(du -h "$DEPLOY_DIR/barter-features.tar.gz" | cut -f1)
    echo "==> Image saved: $DEPLOY_DIR/barter-features.tar.gz ($IMAGE_SIZE)"
fi

# ── Step 3: Transfer to VPS ──────────────────────────────────
echo "==> Transferring files to $VPS_HOST:$VPS_DEPLOY_DIR..."
ssh "$VPS_HOST" "mkdir -p $VPS_DEPLOY_DIR"

# Always transfer compose + env + scripts
scp "$DEPLOY_DIR/compose.yaml" "$VPS_HOST:$VPS_DEPLOY_DIR/"
scp "$DEPLOY_DIR/compose.prod.yaml" "$VPS_HOST:$VPS_DEPLOY_DIR/"
scp "$DEPLOY_DIR/.env.production" "$VPS_HOST:$VPS_DEPLOY_DIR/"
scp "$DEPLOY_DIR/status.sh" "$VPS_HOST:$VPS_DEPLOY_DIR/"
scp "$DEPLOY_DIR/backup.sh" "$VPS_HOST:$VPS_DEPLOY_DIR/"
scp "$DEPLOY_DIR/Containerfile.jupyter" "$VPS_HOST:$VPS_DEPLOY_DIR/"

if [ "$DEPLOY_SERVER" = true ]; then
    scp "$DEPLOY_DIR/barter-server.tar.gz" "$VPS_HOST:$VPS_DEPLOY_DIR/"
fi
if [ "$DEPLOY_FEATURES" = true ]; then
    scp "$DEPLOY_DIR/barter-features.tar.gz" "$VPS_HOST:$VPS_DEPLOY_DIR/"
fi

# ── Step 4: Deploy on VPS ────────────────────────────────────
echo "==> Deploying on VPS..."
ssh "$VPS_HOST" bash <<REMOTE
set -euo pipefail
cd ~/barter

# Copy env if not exists (don't overwrite running config)
if [ ! -f .env ]; then
    cp .env.production .env
    echo "  Created .env from .env.production"
fi

# Load new images
if [ -f barter-server.tar.gz ]; then
    echo "  Loading barter-server image..."
    podman load < barter-server.tar.gz
    rm barter-server.tar.gz
fi
if [ -f barter-features.tar.gz ]; then
    echo "  Loading barter-features image..."
    podman load < barter-features.tar.gz
    rm barter-features.tar.gz
fi

# Build jupyter image on VPS (pure Python, no cross-compile needed)
if [ "$DEPLOY_JUPYTER" = true ]; then
    echo "  Building barter-jupyter image on VPS (this may take a few minutes)..."
    podman build -f Containerfile.jupyter -t barter-jupyter:latest .
fi

# Restart updated services
if [ "$DEPLOY_SERVER" = true ]; then
    echo "  Restarting barter-server..."
    podman-compose -f compose.yaml -f compose.prod.yaml stop barter-server 2>/dev/null || true
    podman rm -f barter-server 2>/dev/null || true
    podman-compose -f compose.yaml -f compose.prod.yaml up -d barter-server
fi

if [ "$DEPLOY_FEATURES" = true ]; then
    echo "  Restarting barter-features..."
    podman-compose -f compose.yaml -f compose.prod.yaml stop barter-features 2>/dev/null || true
    podman rm -f barter-features 2>/dev/null || true
    podman-compose -f compose.yaml -f compose.prod.yaml up -d barter-features
fi

if [ "$DEPLOY_JUPYTER" = true ]; then
    echo "  Restarting jupyter..."
    podman-compose -f compose.yaml -f compose.prod.yaml stop jupyter 2>/dev/null || true
    podman rm -f jupyter 2>/dev/null || true
    podman-compose -f compose.yaml -f compose.prod.yaml up -d jupyter
fi

# Wait for healthcheck
if [ "$DEPLOY_SERVER" = true ] || [ "$DEPLOY_FEATURES" = true ]; then
    echo "  Waiting for health check (up to 120s)..."
    for i in \$(seq 1 12); do
        sleep 10
        if [ -f /data/ipc/collector-heartbeat.json ]; then
            echo "  Heartbeat file found after \${i}0s"
            cat /data/ipc/collector-heartbeat.json | python3 -m json.tool 2>/dev/null || cat /data/ipc/collector-heartbeat.json
            break
        fi
        echo "  Waiting... (\${i}0s)"
    done
fi

if [ "$DEPLOY_JUPYTER" = true ]; then
    echo "  Waiting for jupyter to start (up to 30s)..."
    sleep 5
    if podman ps --format "{{.Names}}" | grep -q "^jupyter\$"; then
        echo "  Jupyter container running"
        podman logs --tail 5 jupyter 2>/dev/null || true
    else
        echo "  WARNING: Jupyter container not running yet"
        podman logs --tail 10 jupyter 2>/dev/null || true
    fi
fi

echo "  Container status:"
podman ps --format "table {{.Names}}\t{{.Status}}\t{{.Ports}}"
REMOTE

echo ""
echo "==> Deploy complete!"
echo "    SSH:    ssh $VPS_HOST"
echo "    Logs:   ssh $VPS_HOST 'podman logs -f barter-server'"
echo "    Status: ssh $VPS_HOST 'bash ~/barter/status.sh'"

# Cleanup local artifacts
rm -f "$DEPLOY_DIR/$SERVER_BINARY"
rm -f "$DEPLOY_DIR/$FEATURES_BINARY"
rm -f "$DEPLOY_DIR/barter-server.tar.gz"
rm -f "$DEPLOY_DIR/barter-features.tar.gz"
