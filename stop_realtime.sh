#!/bin/bash
# Stop all barter-data realtime components

echo "🛑 Stopping barter-data components..."

# Stop server
if pgrep -f "barter-data-server" > /dev/null; then
    echo "   Stopping server..."
    pkill -f "barter-data-server"
    echo "   ✅ Server stopped"
else
    echo "   ℹ️  Server not running"
fi

# Stop TUI
if pgrep -f "barter-data-tui" > /dev/null; then
    echo "   Stopping TUI..."
    pkill -f "barter-data-tui"
    echo "   ✅ TUI stopped"
else
    echo "   ℹ️  TUI not running"
fi

echo "✅ All components stopped"
