#!/bin/bash
# Wrapper to run TUI with automatic terminal cleanup

# Trap to restore terminal on ANY exit
trap 'reset; stty sane; tput rmcup 2>/dev/null; tput cnorm 2>/dev/null' EXIT INT TERM

# Run the TUI
WHALE_THRESHOLD=${WHALE_THRESHOLD:-50000} \
WS_URL=${WS_URL:-ws://127.0.0.1:9001} \
./target/release/market-microstructure 2>tui_clean.log
