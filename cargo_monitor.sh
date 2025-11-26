#!/bin/bash
# Continuous cargo monitoring for Rust development

echo "📦 Cargo Monitor - Watching for changes"
echo "═══════════════════════════════════════"

# Check if cargo-watch is installed
if ! command -v cargo-watch &> /dev/null; then
    echo "⚠️  cargo-watch not found. Installing..."
    cargo install cargo-watch
fi

# Watch for changes and run tests + build
# Shows real-time output of compilation and tests
cargo watch \
    --clear \
    --watch src \
    --watch tests \
    --watch Cargo.toml \
    -x "check --color always" \
    -x "test --color always -- --nocapture" \
    -x "clippy --color always -- -D warnings"
