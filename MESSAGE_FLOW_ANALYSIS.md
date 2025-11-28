# BARTER-DATA-SERVER MESSAGE FLOW ANALYSIS
## Comprehensive Trade Message Routing & WebSocket Handling

---

## EXECUTIVE SUMMARY

The barter-data-server implements a multi-stage pipeline for processing exchange WebSocket messages:

1. **Network Layer**: Raw WebSocket connection to exchange (e.g., Bybit publicTrade)
2. **Deserialization**: Exchange-specific message parsing (JSON -> BybitTradeMessage)
3. **Routing**: Subscription-based message classification and distribution
4. **Transformation**: Exchange-native types -> normalized MarketEvent
5. **Broadcasting**: Transformed events sent to connected TUI clients
6. **Logging**: Strategic logging points for observability

**Current Issue**: Trade messages arrive at network level (confirmed by raw capture) but appear to be hitting an early return condition, preventing them from reaching the application logging and broadcasting layer.

---

## ENTRY POINTS: WHERE WEBSOCKET MESSAGES ENTER THE SERVER

### 1. PRIMARY ENTRY: ExchangeStream Poll Loop
**File**: `barter-integration/src/stream/mod.rs:41-65`
**Function**: `ExchangeStream::poll_next()`

```rust
impl<Protocol, InnerStream, StreamTransformer> Stream
    for ExchangeStream<Protocol, InnerStream, StreamTransformer>
{
    type Item = Result<StreamTransformer::Output, StreamTransformer::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            // 1. Buffer flush (return buffered output if available)
            if let Some(output) = self.buffer.pop_front() {
                return Poll::Ready(Some(output));
            }

            // 2. POLL INNER STREAM for next WebSocket message
            let input = match self.as_mut().project().stream.poll_next(cx) {
                Poll::Ready(Some(input)) => input,  // <-- CRITICAL: Raw message arrives here
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            };

            // 3. PARSE message using Protocol parser
            let exchange_message = match Protocol::parse(input) {
                Some(Ok(exchange_message)) => exchange_message,
                Some(Err(err)) => return Poll::Ready(Some(Err(err.into()))),
                None => continue,  // <-- Safe-to-skip messages (pings, pongs, etc.)
            };

            // 4. TRANSFORM parsed message
            self.transformer
                .transform(exchange_message)
                .into_iter()
                .for_each(|output_result| {
                    self.buffer.push_back(output_result)
                });
        }
    }
}
```

**Key Details**:
- Line 56: `Protocol::parse(input)` attempts to deserialize the WebSocket message
- Returns `None` for safe-to-skip messages (continues loop)
- Returns `Some(Ok(...))` for successfully parsed messages
- Returns `Some(Err(...))` for parsing errors
- Line 69-75: Transform parsed messages into output events

### 2. WEBSOCKET STREAM SOURCE
**File**: `barter-data/src/lib.rs:250-282`
**Function**: `MarketStream::init()` implementation for ExchangeWsStream

```rust
// Split WebSocket into WsStream & WsSink components
let (ws_sink, ws_stream) = websocket.split();

// Initialize Transformer
let mut transformer =
    Transformer::init(instrument_map, &initial_snapshots, ws_sink_tx).await?;

// Process buffered events from subscription validation
let mut processed = process_buffered_events::<Parser, Transformer>(
    &mut transformer,
    buffered_websocket_events,
);

Ok(ExchangeWsStream::new(ws_stream, transformer, processed))
```

**Key Details**:
- `ws_stream` becomes the inner stream for ExchangeStream
- `transformer` is initialized with instrument_map (maps subscription_id -> instrument)
- `buffered_websocket_events` contains messages received during subscription validation

---

## MESSAGE DESERIALIZATION: Exchange-Specific Parsing

### Bybit Trade Message Deserialization
**File**: `barter-data/src/exchange/bybit/trade.rs:14-39`

```rust
/// Terse type alias for an BybitTrade real-time trades WebSocket message
pub type BybitTrade = BybitPayload<Vec<BybitTradeInner>>;

/// Messages received on the Bybit trade stream
#[derive(Clone, Debug)]
pub enum BybitTradeMessage {
    Ignore,
    Payload(BybitTrade),
}

impl<'de> Deserialize<'de> for BybitTradeMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;

        match value.get("topic") {
            // CRITICAL: Checks for "topic" field presence
            Some(topic) if topic.is_string() => {
                let raw = serde_json::to_string(&value).map_err(serde::de::Error::custom)?;
                serde_json::from_str(&raw)
                    .map(BybitTradeMessage::Payload)  // <-- Creates Payload variant
                    .map_err(serde::de::Error::custom)
            }
            // CRITICAL: Returns Ignore if no "topic" field!
            _ => Ok(BybitTradeMessage::Ignore),
        }
    }
}
```

**CRITICAL OBSERVATION**: 
- If the JSON message doesn't have a "topic" field, it returns `BybitTradeMessage::Ignore`
- This is a **POTENTIAL EARLY RETURN POINT** that could silently drop messages

### Topic Parsing
**File**: `barter-data/src/exchange/bybit/message.rs:58-95`

```rust
pub fn de_message_subscription_id<'de, D>(deserializer: D) -> Result<SubscriptionId, D::Error>
where
    D: serde::de::Deserializer<'de>,
{
    let input = <&str as serde::Deserialize>::deserialize(deserializer)?;
    let mut tokens = input.split('.');

    match (tokens.next(), tokens.next(), tokens.next()) {
        // MATCHES: "publicTrade.BTCUSDT"
        (Some("publicTrade"), Some(market), None) => Ok(SubscriptionId::from(format!(
            "{}|{market}",
            BybitChannel::TRADES.0  // "publicTrade"
        ))),
        // Expected format: "publicTrade|BTCUSDT"
        _ => Err(Error::invalid_value(
            Unexpected::Str(input),
            &"invalid message type expected pattern: <type>.<symbol>",
        )),
    }
}
```

**Expected Topic Format**: `"publicTrade.BTCUSDT"` -> Subscription ID: `"publicTrade|BTCUSDT"`

---

## ROUTING LOGIC: Message Classification & Distribution

### 1. Subscription ID Extraction
**File**: `barter-data/src/exchange/bybit/trade.rs:113-120`

```rust
impl Identifier<Option<SubscriptionId>> for BybitTradeMessage {
    fn id(&self) -> Option<SubscriptionId> {
        match self {
            BybitTradeMessage::Payload(payload) => payload.id(),
            // CRITICAL: Ignore messages return None!
            BybitTradeMessage::Ignore => None,
        }
    }
}
```

### 2. Transformer Routing
**File**: `barter-data/src/transformer/stateless.rs:64-84`

```rust
fn transform(&mut self, input: Self::Input) -> Self::OutputIter {
    // Step 1: Extract subscription ID from message
    let subscription_id = match input.id() {
        Some(subscription_id) => subscription_id,
        // CRITICAL: Returns empty vec if no subscription_id!
        None => return vec![],  // <-- EARLY RETURN: Message is dropped here!
    };

    // Step 2: Find instrument associated with this subscription
    match self.instrument_map.find(&subscription_id) {
        Ok(instrument) => {
            // Transform message to MarketEvent
            MarketIter::<InstrumentKey, Kind::Event>::from((
                Exchange::ID,
                instrument.clone(),
                input,
            ))
            .0
        }
        // Step 3: Error if subscription not in map
        Err(unidentifiable) => vec![Err(DataError::from(unidentifiable))],
    }
}
```

**CRITICAL FAILURE POINTS**:
1. **Line 66-69**: If `input.id()` returns `None`, function returns empty vec!
   - For BybitTradeMessage::Ignore → returns None → empty vec → message dropped
2. **Line 72-82**: If subscription_id not in instrument_map → returns error
   - Messages for unsubscribed instruments are errors (not dropped silently)

---

## TRADE-SPECIFIC HANDLING: Where Trade Messages Should Be Processed

### 1. Trade Message Conversion
**File**: `barter-data/src/exchange/bybit/trade.rs:81-111`

```rust
impl<InstrumentKey: Clone> From<(ExchangeId, InstrumentKey, BybitTradeMessage)>
    for MarketIter<InstrumentKey, PublicTrade>
{
    fn from(
        (exchange, instrument, message): (ExchangeId, InstrumentKey, BybitTradeMessage),
    ) -> Self {
        match message {
            // CRITICAL: Ignore variant creates empty iter!
            BybitTradeMessage::Ignore => Self(vec![]),
            BybitTradeMessage::Payload(trades) => Self(
                trades
                    .data
                    .into_iter()
                    .map(|trade| {
                        Ok(MarketEvent {
                            time_exchange: trade.time,
                            time_received: Utc::now(),
                            exchange,
                            instrument: instrument.clone(),
                            kind: PublicTrade {
                                id: trade.id,
                                price: trade.price,
                                amount: trade.amount,
                                side: trade.side,
                            },
                        })
                    })
                    .collect(),
            ),
        }
    }
}
```

### 2. Application-Level Trade Logging
**File**: `barter-data-server/src/main.rs:130-153`

```rust
Ok(market_event) => {
    // Debug logging for large spot trades
    if let DataKind::Trade(trade) = &market_event.kind {
        let spot_log_threshold = std::env::var("SPOT_LOG_THRESHOLD")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50_000.0);
        let notional = trade.price * trade.amount;
        let is_spot =
            matches!(market_event.instrument.kind, MarketDataInstrumentKind::Spot);
        if is_spot && notional >= spot_log_threshold {
            // THIS IS WHERE TRADES SHOULD APPEAR IN SERVER_DEBUG.LOG
            info!(
                "SPOT TRADE >=50k {} {}/{} @ {} qty {} notional {} side {:?}",
                market_event.exchange,
                market_event.instrument.base,
                market_event.instrument.quote,
                trade.price,
                trade.amount,
                notional,
                trade.side
            );
        }
    }
}
```

**Expected Behavior**: 
- Trades arrive as `DataKind::Trade(trade)`
- Filtered by notional value (>= 50k by default)
- Logged to server_debug.log with "SPOT TRADE >=50k" prefix

**Actual Observation**:
- Early logs (01:50-01:52) show "SPOT TRADE >=50k" entries
- Recent logs (02:17+) show ZERO trade entries
- This suggests trades are being dropped before reaching this point

---

## LOGGING POINTS: Where Trades Should Appear in server_debug.log

### 1. Market Stream Initialization
**File**: `barter-data/src/streams/consumer.rs:64-70`
```
MarketStream with auto reconnect initialising
subscriptions = {exchange_subscriptions}
```

### 2. WebSocket Subscription Validation
**File**: `barter-data/src/subscriber/validator.rs:64`
```
validated exchange WebSocket subscriptions
```

### 3. Subscription Confirmation
**File**: `barter-data/src/subscriber/validator.rs:87-93`
```
received valid Ok subscription response
success_responses = {count}
expected_responses = {count}
```

### 4. Trade Event Logging (PRIMARY)
**File**: `barter-data-server/src/main.rs:142-151`
```
SPOT TRADE >=50k {exchange} {base}/{quote} @ {price} qty {amount} notional {notional} side {side}
```

### 5. WebSocket Client Broadcasting
**File**: `barter-data-server/src/main.rs:207-214`
```
tx.send(message) -> count of receivers
```

---

## POTENTIAL FAILURE POINTS: Where Messages Could Be Dropped or Filtered

### FAILURE POINT #1: Deserialization Returns Ignore
**Location**: `barter-data/src/exchange/bybit/trade.rs:30-37`
**Trigger**: Message missing "topic" field
**Impact**: Message converted to `BybitTradeMessage::Ignore`

```rust
match value.get("topic") {
    Some(topic) if topic.is_string() => { /* Process */ }
    _ => Ok(BybitTradeMessage::Ignore),  // <-- DROPS MESSAGE
}
```

**Detection**: Check if raw WebSocket capture shows "topic" field in trade messages
**Hypothesis**: If Bybit raw format doesn't match expected "topic" key, all trades dropped here

---

### FAILURE POINT #2: Identifier Returns None
**Location**: `barter-data/src/exchange/bybit/trade.rs:113-120`
**Trigger**: BybitTradeMessage::Ignore variant
**Impact**: Transformer routing skips message

```rust
impl Identifier<Option<SubscriptionId>> for BybitTradeMessage {
    fn id(&self) -> Option<SubscriptionId> {
        match self {
            BybitTradeMessage::Payload(payload) => payload.id(),
            BybitTradeMessage::Ignore => None,  // <-- No ID = No routing
        }
    }
}
```

---

### FAILURE POINT #3: Subscription Not in Instrument Map
**Location**: `barter-data/src/transformer/stateless.rs:66-69`
**Trigger**: Subscription ID not registered during initialization
**Impact**: Early return, no error logged

```rust
let subscription_id = match input.id() {
    Some(subscription_id) => subscription_id,
    None => return vec![],  // <-- Silent drop!
};
```

**Hypothesis**: Server subscribed to wrong symbol or subscription ID mismatch
**Check**: Server initialization at `barter-data-server/src/main.rs:345-463`

---

### FAILURE POINT #4: Subscription ID Parse Error
**Location**: `barter-data/src/exchange/bybit/message.rs:62-95`
**Trigger**: Topic format doesn't match expected pattern
**Impact**: Deserialization error, message dropped with error

```rust
match (tokens.next(), tokens.next(), tokens.next()) {
    (Some("publicTrade"), Some(market), None) => Ok(/* ... */),
    _ => Err(Error::invalid_value(/* ... */)),  // <-- Deserialize error
}
```

**Expected Format**: `"publicTrade.BTCUSDT"` (exactly 2 components)
**Risk**: Extra dots or different topic names would fail here

---

### FAILURE POINT #5: Broadcast Channel Overflow
**Location**: `barter-data-server/src/main.rs:207-224`
**Trigger**: More messages than buffer capacity
**Impact**: Messages skipped with warning

```rust
match tx.send(message) {
    Ok(count) => { /* Success */ }
    Err(e) => {
        warn!("Failed to broadcast liquidation: {:?}", e);
    }
}
```

---

## ROOT CAUSE HYPOTHESIS

Based on the code analysis:

**Most Likely**: Deserialization failure at `BybitTradeMessage::deserialize()`
- Raw Bybit publicTrade messages may not have "topic" field at root level
- Or topic field may be nested differently than expected
- Results in `BybitTradeMessage::Ignore` variant
- Silently dropped in transformer

**Supporting Evidence**:
- Early logs (01:50-01:52) had SPOT TRADE entries
- Suggests subscription was working at some point
- Recent absence could indicate:
  - Bybit API change in message format
  - Server reconnection with different message schema
  - Subscription type mismatch

---

## ABSOLUTE CODE ENTRY POINT LOCATIONS

### Message Reception Chain:
1. **Network RX**: `tokio_tungstenite::WebSocket::next()` (external crate)
2. **Async Poll**: `barter-integration/src/stream/mod.rs:49` - `ExchangeStream::poll_next()`
3. **Parser Invoke**: `barter-integration/src/stream/mod.rs:56` - `Protocol::parse(input)`
4. **Deserialize**: `barter-data/src/exchange/bybit/trade.rs:23` - `BybitTradeMessage::deserialize()`
5. **Extract Topic**: `barter-data/src/exchange/bybit/message.rs:62` - `de_message_subscription_id()`
6. **Route/Transform**: `barter-data/src/transformer/stateless.rs:64` - `StatelessTransformer::transform()`
7. **Convert Event**: `barter-data/src/exchange/bybit/trade.rs:81` - `From<(ExchangeId, InstrumentKey, BybitTradeMessage)>`
8. **Application Log**: `barter-data-server/src/main.rs:142` - Trade logging point
9. **Broadcast**: `barter-data-server/src/main.rs:207` - tx.send(message)

---

## MESSAGE FLOW DIAGRAM

```
┌─────────────────────────────────────────────────────────────────┐
│ BYBIT WEBSOCKET (Network Layer)                                 │
│ Raw: {"topic":"publicTrade.BTCUSDT","type":"snapshot","data":[]}│
└─────────────────┬───────────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────────────┐
│ ExchangeStream::poll_next() [barter-integration/stream/mod.rs]  │
│ Polls inner ws_stream, receives WsMessage                       │
└─────────────────┬───────────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────────────┐
│ Protocol::parse() [WebSocketSerdeParser]                        │
│ Deserializes JSON to Protocol::Message type                     │
└─────────────────┬───────────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────────────┐
│ BybitTradeMessage::deserialize()                                │
│ [barter-data/exchange/bybit/trade.rs:23-40]                     │
│ ┌─────────────────────────────────────────┐                     │
│ │ Check value.get("topic")                │                     │
│ │ YES → BybitTradeMessage::Payload        │ CRITICAL POINT!    │
│ │ NO  → BybitTradeMessage::Ignore ⚠️      │                     │
│ └─────────────────────────────────────────┘                     │
└─────────────────┬───────────────────────────────────────────────┘
                  │
                  ▼ (Only Payload variant)
┌─────────────────────────────────────────────────────────────────┐
│ StatelessTransformer::transform()                               │
│ [barter-data/transformer/stateless.rs:64-84]                    │
│ ┌─────────────────────────────────────────┐                     │
│ │ input.id() → SubscriptionId             │                     │
│ │ Find in instrument_map                  │                     │
│ │ OR → return vec![] ⚠️ (Silent drop!)    │                     │
│ └─────────────────────────────────────────┘                     │
└─────────────────┬───────────────────────────────────────────────┘
                  │
                  ▼ (Only if in map)
┌─────────────────────────────────────────────────────────────────┐
│ BybitTradeMessage→From<MarketIter<PublicTrade>>                │
│ [barter-data/exchange/bybit/trade.rs:81-111]                    │
│ Converts to MarketEvent<PublicTrade>                            │
└─────────────────┬───────────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────────────┐
│ barter-data-server Main Loop                                    │
│ [barter-data-server/src/main.rs:124-237]                        │
│ ┌──────────────────────────────────────────────┐                │
│ │ if let DataKind::Trade(trade) { ... }        │                │
│ │ Log: "SPOT TRADE >=50k ..." ← SERVER LOG     │ ← YOU ARE HERE!│
│ │ tx.send(message) → Broadcast to clients      │                │
│ └──────────────────────────────────────────────┘                │
└─────────────────┬───────────────────────────────────────────────┘
                  │
                  ▼
┌─────────────────────────────────────────────────────────────────┐
│ WebSocket Clients Receive Trade Events                          │
│ (TUI applications consume messages)                             │
└─────────────────────────────────────────────────────────────────┘
```

---

## SUBSCRIPTION CONFIRMATION/ACK MECHANISM

### 1. Subscription Request
**File**: `barter-data/src/exchange/bybit/mod.rs:115-128`

```rust
fn requests(exchange_subs: Vec<ExchangeSub<Self::Channel, Self::Market>>) -> Vec<WsMessage> {
    let stream_names = exchange_subs
        .into_iter()
        .map(|sub| format!("{}.{}", sub.channel.as_ref(), sub.market.as_ref(),))
        .collect::<Vec<String>>();

    vec![WsMessage::text(
        serde_json::json!({
            "op": "subscribe",
            "args": stream_names
        })
        .to_string(),
    )]
}
```

**Sends Format**:
```json
{
    "op": "subscribe",
    "args": ["publicTrade.BTCUSDT", "publicTrade.ETHUSDT", "publicTrade.SOLUSDT"]
}
```

### 2. Expected Response Format
**File**: `barter-data/src/exchange/bybit/subscription.rs:27-42`

```rust
#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Deserialize, Serialize)]
pub struct BybitResponse {
    pub success: bool,
    #[serde(default)]
    pub ret_msg: BybitReturnMessage,
}
```

**Expected Format**:
```json
{
    "success": true,
    "ret_msg": "subscribe",
    "conn_id": "...",
    "req_id": "...",
    "op": "subscribe"
}
```

### 3. Subscription Validation
**File**: `barter-data/src/subscriber/validator.rs:38-119`

Loop continues until:
- `success_responses == expected_responses` (1 ACK per subscription batch)
- Or 30-second timeout
- Or subscription error

---

## SUMMARY TABLE: Message Pipeline Stages

| Stage | Location | Input Type | Output Type | Failure Mode |
|-------|----------|-----------|------------|--------------|
| Network RX | tokio_tungstenite | Raw WebSocket | WsMessage::Text(JSON) | Connection drop |
| Parse | stream/mod.rs:56 | WsMessage | BybitTradeMessage | Protocol::parse returns None |
| Deserialize | trade.rs:23 | JSON string | BybitTradeMessage::Ignore or Payload | No "topic" field → Ignore |
| Extract ID | message.rs:62 | topic string | SubscriptionId | Invalid format → Error |
| Route | stateless.rs:66 | BybitTradeMessage | Returns Option or vec | Ignore variant → None → empty vec |
| Transform | bybit.rs:81 | Input Enum | MarketEvent vector | Ignore variant → empty vec |
| Application | main.rs:142 | MarketEvent | Log entry | Trade kind check fails |
| Broadcast | main.rs:207 | MarketEventMessage | Send count | Channel full → warning |

