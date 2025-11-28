#!/bin/bash
# Start barter-data realtime monitoring

echo "🚀 Starting barter-data realtime monitoring..."

# Check if server already running
if pgrep -f "barter-data-server" > /dev/null; then
    echo "   ⚠️  Server already running"
else
    echo "   Starting server in background..."
    cargo run -p barter-data-server > /tmp/barter-server.log 2>&1 &
    sleep 3
    echo "   ✅ Server started (logs: /tmp/barter-server.log)"
fi

echo ""
echo "📊 Launching TUI..."
echo "   Press 'q' or 'Esc' to quit TUI"
echo ""
sleep 1

# Launch TUI in foreground
cargo run -p barter-data-tui
