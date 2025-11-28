#!/bin/bash
# Granular trade analysis by Exchange → Market Type → Token
# Analyzes log file to show exactly which combinations work/fail

if [ -z "$1" ]; then
    LOG_FILE="/tmp/bybit_trade_diagnostic.log"
else
    LOG_FILE="$1"
fi

if [ ! -f "$LOG_FILE" ]; then
    echo "Error: Log file not found: $LOG_FILE"
    echo "Usage: $0 [log_file]"
    exit 1
fi

echo "==================================================================="
echo "GRANULAR TRADE ANALYSIS - EXCHANGE → MARKET → TOKEN"
echo "==================================================================="
echo "Log file: $LOG_FILE"
echo ""

# Function to extract symbol/market from trade debug messages
analyze_exchange_trades() {
    local exchange=$1
    local debug_pattern=$2

    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "📊 ${exchange^^}"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo ""

    # Total messages for this exchange
    total=$(grep -c "$debug_pattern" "$LOG_FILE" || echo 0)

    if [ "$total" -eq 0 ]; then
        echo "❌ NO MESSAGES RECEIVED"
        echo ""
        return
    fi

    echo "Total messages: $total"
    echo ""

    # Extract unique symbols/markets from trade messages
    echo "Breakdown by symbol/market:"
    echo ""

    # Try to extract symbols from the debug messages
    grep "$debug_pattern" "$LOG_FILE" | while read -r line; do
        # Extract symbol patterns
        # Bybit: "Trade: BTCUSDT @ ..."
        # Binance: "Trade: @trade|BTCUSDT @ ..."
        # OKX: "Trade: 12345 @ ..." (need different approach)

        if [[ "$line" =~ BTC|btc ]]; then
            echo "BTC"
        elif [[ "$line" =~ ETH|eth ]]; then
            echo "ETH"
        elif [[ "$line" =~ SOL|sol ]]; then
            echo "SOL"
        fi
    done | sort | uniq -c | while read -r count token; do
        echo "  $token: $count trades"
    done

    echo ""
}

# Analyze BYBIT in detail
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 BYBIT (Detailed Analysis)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

bybit_total=$(grep -c '\[BYBIT TRADE DEBUG\]' "$LOG_FILE" || echo 0)
bybit_with_topic=$(grep -c 'has .topic. field: true' "$LOG_FILE" || echo 0)
bybit_without_topic=$(grep -c 'has .topic. field: false' "$LOG_FILE" || echo 0)
bybit_dropped=$(grep -c 'MESSAGE WITHOUT TOPIC - WILL BE DROPPED' "$LOG_FILE" || echo 0)
bybit_success=$(grep -c 'Successfully deserialized' "$LOG_FILE" || echo 0)
bybit_converted=$(grep -c 'Converting .* Bybit trades to MarketEvents' "$LOG_FILE" || echo 0)

echo "Total messages: $bybit_total"
echo "  With 'topic' field: $bybit_with_topic"
echo "  WITHOUT 'topic' field: $bybit_without_topic"
echo "  Dropped (Ignore): $bybit_dropped"
echo "  Successfully deserialized: $bybit_success"
echo "  Converted to MarketEvents: $bybit_converted"
echo ""

if [ "$bybit_dropped" -gt 0 ]; then
    echo "⚠️  CRITICAL: $bybit_dropped messages being DROPPED"
    echo ""
    echo "Sample dropped message (showing token/market):"
    grep -A 15 'MESSAGE WITHOUT TOPIC - WILL BE DROPPED' "$LOG_FILE" | head -30 | grep -E '"s"|"symbol"|USDT'
    echo ""
fi

# Try to extract symbols from successfully processed Bybit trades
if [ "$bybit_converted" -gt 0 ]; then
    echo "Successfully processed trades by symbol:"
    grep 'Trade:.*@' "$LOG_FILE" | grep BYBIT | while read -r line; do
        if echo "$line" | grep -iq 'btc'; then echo "BTC"
        elif echo "$line" | grep -iq 'eth'; then echo "ETH"
        elif echo "$line" | grep -iq 'sol'; then echo "SOL"
        fi
    done | sort | uniq -c | while read -r count token; do
        echo "  $token: $count trades"
    done
    echo ""
fi

# Analyze dropped messages by symbol
if [ "$bybit_dropped" -gt 0 ]; then
    echo "Dropped messages by symbol (from JSON):"
    grep -A 20 'Full message:' "$LOG_FILE" | grep '"s"' | sed 's/.*"s"[ :]*"\([^"]*\)".*/\1/' | while read -r symbol; do
        if echo "$symbol" | grep -iq 'btc'; then echo "BTC"
        elif echo "$symbol" | grep -iq 'eth'; then echo "ETH"
        elif echo "$symbol" | grep -iq 'sol'; then echo "SOL"
        fi
    done | sort | uniq -c | while read -r count token; do
        echo "  $token: $count trades DROPPED"
    done
    echo ""
fi

echo ""

# Analyze BINANCE
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 BINANCE"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

binance_total=$(grep -c '\[BINANCE TRADE DEBUG\]' "$LOG_FILE" || echo 0)
binance_converted=$(grep -c 'BINANCE TRADE DEBUG\] Converting Binance trade' "$LOG_FILE" || echo 0)

echo "Total messages: $binance_total"
echo "Converted to MarketEvents: $binance_converted"
echo ""

if [ "$binance_total" -eq 0 ]; then
    echo "❌ NO MESSAGES RECEIVED"
    echo ""
else
    echo "Trades by symbol:"
    grep '\[BINANCE TRADE DEBUG\]' "$LOG_FILE" | while read -r line; do
        if echo "$line" | grep -iq 'btc'; then echo "BTC"
        elif echo "$line" | grep -iq 'eth'; then echo "ETH"
        elif echo "$line" | grep -iq 'sol'; then echo "SOL"
        fi
    done | sort | uniq -c | while read -r count token; do
        echo "  $token: $count trades"
    done
    echo ""
fi

echo ""

# Analyze OKX
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 OKX"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

okx_total=$(grep -c '\[OKX TRADE DEBUG\]' "$LOG_FILE" || echo 0)
okx_converted=$(grep 'OKX TRADE DEBUG\] Converting' "$LOG_FILE" | wc -l | tr -d ' ')

echo "Total messages: $okx_total"
echo "Converted to MarketEvents: $okx_converted"
echo ""

if [ "$okx_total" -eq 0 ]; then
    echo "❌ NO MESSAGES RECEIVED"
    echo ""
else
    echo "Trades by symbol:"
    grep '\[OKX TRADE DEBUG\]' "$LOG_FILE" | while read -r line; do
        if echo "$line" | grep -iq 'btc'; then echo "BTC"
        elif echo "$line" | grep -iq 'eth'; then echo "ETH"
        elif echo "$line" | grep -iq 'sol'; then echo "SOL"
        fi
    done | sort | uniq -c | while read -r count token; do
        echo "  $token: $count trades"
    done
    echo ""
fi

echo ""

# Analyze SPOT vs PERPETUAL from server logs
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 SPOT TRADES (from server logs - large trades >=50k)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

spot_total=$(grep -c 'SPOT TRADE >=50k' "$LOG_FILE" || echo 0)
echo "Total spot whale trades logged: $spot_total"
echo ""

if [ "$spot_total" -gt 0 ]; then
    echo "Breakdown by exchange:"
    for exchange in BinanceSpot BybitSpot Okx; do
        count=$(grep "SPOT TRADE >=50k $exchange" "$LOG_FILE" | wc -l | tr -d ' ')
        echo "  $exchange: $count trades"
    done
    echo ""

    echo "Breakdown by token (spot):"
    for token in btc eth sol; do
        count=$(grep "SPOT TRADE >=50k" "$LOG_FILE" | grep -i "$token" | wc -l | tr -d ' ')
        if [ "$count" -gt 0 ]; then
            echo "  ${token^^}: $count trades"
        fi
    done
    echo ""
else
    echo "❌ No large spot trades detected (all trades < $50k or not flowing)"
    echo ""
fi

echo ""

# Summary matrix
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 SUMMARY MATRIX"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "Exchange     | Messages | Converted | Status"
echo "-------------|----------|-----------|----------------"
printf "%-12s | %8d | %9d | " "Bybit" "$bybit_total" "$bybit_converted"
if [ "$bybit_dropped" -gt 0 ]; then
    echo "❌ DROPPING ($bybit_dropped dropped)"
elif [ "$bybit_total" -eq 0 ]; then
    echo "❌ NO MESSAGES"
else
    echo "✅ WORKING"
fi

printf "%-12s | %8d | %9d | " "Binance" "$binance_total" "$binance_converted"
if [ "$binance_total" -eq 0 ]; then
    echo "❌ NO MESSAGES"
else
    echo "✅ WORKING"
fi

printf "%-12s | %8d | %9d | " "OKX" "$okx_total" "$okx_converted"
if [ "$okx_total" -eq 0 ]; then
    echo "❌ NO MESSAGES"
else
    echo "✅ WORKING"
fi

echo ""
echo "==================================================================="
echo "END OF GRANULAR ANALYSIS"
echo "==================================================================="
