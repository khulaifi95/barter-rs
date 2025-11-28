#!/bin/bash
# Run the enhanced Scalper TUI with terminal cleanup

# Trap to restore terminal on ANY exit
trap 'reset; stty sane; tput rmcup 2>/dev/null; tput cnorm 2>/dev/null' EXIT INT TERM

# Configuration
WHALE_THRESHOLD=${WHALE_THRESHOLD:-50000}  # Min whale size ($50K default)
WS_URL=${WS_URL:-ws://127.0.0.1:9001}      # WebSocket data source

echo "Starting Scalper TUI..."
echo "  Whale threshold: \$${WHALE_THRESHOLD}"
echo "  Data source: ${WS_URL}"
echo ""
echo "Hotkeys: [B]TC [E]TH [S]OL | [Tab] cycle | [q] quit"
echo ""

# Run the scalper
WHALE_THRESHOLD=$WHALE_THRESHOLD \
WS_URL=$WS_URL \
./target/release/scalper 2>/dev/null
