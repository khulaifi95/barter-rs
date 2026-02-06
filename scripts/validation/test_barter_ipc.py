#!/usr/bin/env python3
"""
Barter IPC Protocol Test

Tests the MessagePack IPC protocol used by barter-data-server for UDS/TCP streaming.
This validates that the protocol can be decoded by a Python client, which is what
a Nautilus adapter would need to implement.

Protocol:
- 4-byte big-endian length prefix
- MessagePack encoded payload
- Payload structure: {"Event": {market_event_message}}

Usage:
    # Test with running barter-data-server
    python scripts/validation/test_barter_ipc.py --uds /tmp/barter-data.sock
    python scripts/validation/test_barter_ipc.py --tcp 127.0.0.1:9002

    # Test MessagePack decoding only (no server required)
    python scripts/validation/test_barter_ipc.py --test-decode

Prerequisites:
    uv pip install msgpack
"""

import argparse
import socket
import struct
import sys
from datetime import datetime

try:
    import msgpack
except ImportError:
    print("ERROR: msgpack not installed. Run: uv pip install msgpack")
    sys.exit(1)


def decode_message(data: bytes) -> dict:
    """Decode a MessagePack message."""
    return msgpack.unpackb(data, raw=False, timestamp=3)  # timestamp=3 for datetime


def create_sample_message() -> bytes:
    """Create a sample MessagePack message matching barter-data-server format."""
    # This matches the UdsMessageRef::Event(MarketEventMessage) format
    message = {
        "Event": {
            "time_exchange": datetime.utcnow().isoformat() + "Z",
            "time_received": datetime.utcnow().isoformat() + "Z",
            "exchange": "BinanceFuturesUsd",
            "instrument": {
                "base": "btc",
                "quote": "usdt",
                "kind": "perpetual"
            },
            "kind": "trade",
            "data": {
                "id": "1234567890",
                "price": 100000.50,
                "amount": 0.001,
                "side": "Buy"
            }
        }
    }
    return msgpack.packb(message, datetime=True)


def frame_message(payload: bytes) -> bytes:
    """Add 4-byte big-endian length prefix to payload."""
    length = len(payload)
    return struct.pack(">I", length) + payload


def unframe_message(data: bytes) -> tuple[bytes, bytes]:
    """Extract payload from framed message. Returns (payload, remaining)."""
    if len(data) < 4:
        raise ValueError("Insufficient data for length prefix")
    length = struct.unpack(">I", data[:4])[0]
    if len(data) < 4 + length:
        raise ValueError(f"Insufficient data: need {4 + length}, have {len(data)}")
    return data[4:4+length], data[4+length:]


def test_decode_roundtrip():
    """Test MessagePack encode/decode roundtrip."""
    print("=== Testing MessagePack Decode ===")

    # Create and encode sample message
    payload = create_sample_message()
    print(f"Encoded payload size: {len(payload)} bytes")

    # Frame the message
    framed = frame_message(payload)
    print(f"Framed message size: {len(framed)} bytes")

    # Unframe and decode
    extracted, remaining = unframe_message(framed)
    assert len(remaining) == 0, "Should have no remaining data"

    decoded = decode_message(extracted)
    print(f"Decoded message type: {type(decoded)}")
    print(f"Message keys: {decoded.keys()}")

    # Verify structure
    assert "Event" in decoded, "Should have Event key"
    event = decoded["Event"]
    assert "time_exchange" in event, "Should have time_exchange"
    assert "exchange" in event, "Should have exchange"
    assert "instrument" in event, "Should have instrument"
    assert "kind" in event, "Should have kind"
    assert "data" in event, "Should have data"

    print("\nDecoded event:")
    print(f"  Exchange: {event['exchange']}")
    print(f"  Instrument: {event['instrument']}")
    print(f"  Kind: {event['kind']}")
    print(f"  Data: {event['data']}")

    print("\n✓ MessagePack decode test PASSED")
    return True


def test_uds_connection(uds_path: str, timeout: float = 5.0, max_messages: int = 5):
    """Test UDS connection to barter-data-server."""
    print(f"\n=== Testing UDS Connection: {uds_path} ===")

    try:
        sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        sock.settimeout(timeout)
        sock.connect(uds_path)
        print(f"Connected to {uds_path}")
    except FileNotFoundError:
        print(f"ERROR: UDS socket not found: {uds_path}")
        print("Is barter-data-server running with UDS_ENABLED=true?")
        return False
    except socket.timeout:
        print(f"ERROR: Connection timeout")
        return False
    except Exception as e:
        print(f"ERROR: Connection failed: {e}")
        return False

    try:
        buffer = b""
        messages_received = 0

        while messages_received < max_messages:
            # Receive data
            data = sock.recv(4096)
            if not data:
                print("Connection closed by server")
                break

            buffer += data

            # Process complete messages
            while len(buffer) >= 4:
                length = struct.unpack(">I", buffer[:4])[0]
                if len(buffer) < 4 + length:
                    break  # Need more data

                payload = buffer[4:4+length]
                buffer = buffer[4+length:]

                try:
                    decoded = decode_message(payload)
                    messages_received += 1

                    if "Event" in decoded:
                        event = decoded["Event"]
                        print(f"  [{messages_received}] {event.get('kind', 'unknown')}: "
                              f"{event.get('exchange', '?')}/{event.get('instrument', {}).get('base', '?')}")
                    else:
                        print(f"  [{messages_received}] Unknown message type: {decoded.keys()}")

                except Exception as e:
                    print(f"  Decode error: {e}")

        print(f"\n✓ Received and decoded {messages_received} messages")
        return messages_received > 0

    finally:
        sock.close()


def test_tcp_connection(host: str, port: int, timeout: float = 5.0, max_messages: int = 5):
    """Test TCP connection to barter-data-server."""
    print(f"\n=== Testing TCP Connection: {host}:{port} ===")

    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(timeout)
        sock.connect((host, port))
        print(f"Connected to {host}:{port}")
    except socket.timeout:
        print(f"ERROR: Connection timeout")
        return False
    except ConnectionRefusedError:
        print(f"ERROR: Connection refused - is barter-data-server running with TCP_ENABLED=true?")
        return False
    except Exception as e:
        print(f"ERROR: Connection failed: {e}")
        return False

    try:
        buffer = b""
        messages_received = 0

        while messages_received < max_messages:
            data = sock.recv(4096)
            if not data:
                print("Connection closed by server")
                break

            buffer += data

            while len(buffer) >= 4:
                length = struct.unpack(">I", buffer[:4])[0]
                if len(buffer) < 4 + length:
                    break

                payload = buffer[4:4+length]
                buffer = buffer[4+length:]

                try:
                    decoded = decode_message(payload)
                    messages_received += 1

                    if "Event" in decoded:
                        event = decoded["Event"]
                        print(f"  [{messages_received}] {event.get('kind', 'unknown')}: "
                              f"{event.get('exchange', '?')}/{event.get('instrument', {}).get('base', '?')}")
                    else:
                        print(f"  [{messages_received}] Unknown: {decoded.keys()}")

                except Exception as e:
                    print(f"  Decode error: {e}")

        print(f"\n✓ Received and decoded {messages_received} messages")
        return messages_received > 0

    finally:
        sock.close()


def main():
    parser = argparse.ArgumentParser(description="Test Barter IPC protocol")
    parser.add_argument("--uds", help="UDS socket path (e.g., /tmp/barter-data.sock)")
    parser.add_argument("--tcp", help="TCP address (e.g., 127.0.0.1:9002)")
    parser.add_argument("--test-decode", action="store_true",
                       help="Test MessagePack decoding only (no server required)")
    parser.add_argument("--timeout", type=float, default=5.0,
                       help="Connection timeout in seconds")
    parser.add_argument("--max-messages", type=int, default=5,
                       help="Maximum messages to receive")

    args = parser.parse_args()

    print("=" * 60)
    print("Barter IPC Protocol Test")
    print("=" * 60)

    results = []

    # Always test decode
    if args.test_decode or (not args.uds and not args.tcp):
        results.append(("MessagePack Decode", test_decode_roundtrip()))

    # Test UDS if specified
    if args.uds:
        results.append(("UDS Connection",
                       test_uds_connection(args.uds, args.timeout, args.max_messages)))

    # Test TCP if specified
    if args.tcp:
        try:
            host, port = args.tcp.rsplit(":", 1)
            port = int(port)
            results.append(("TCP Connection",
                           test_tcp_connection(host, port, args.timeout, args.max_messages)))
        except ValueError:
            print(f"ERROR: Invalid TCP address format: {args.tcp}")
            print("Expected format: host:port (e.g., 127.0.0.1:9002)")
            sys.exit(1)

    # Summary
    print("\n" + "=" * 60)
    print("SUMMARY")
    print("=" * 60)

    all_passed = True
    for name, passed in results:
        status = "PASSED" if passed else "FAILED"
        print(f"  {name}: {status}")
        if not passed:
            all_passed = False

    if all_passed:
        print("\n✓ All tests passed!")
        print("\nThe Barter IPC protocol can be decoded by Python/msgpack.")
        print("A Nautilus adapter would need to:")
        print("  1. Connect to UDS/TCP socket")
        print("  2. Read 4-byte length prefix + MessagePack payload")
        print("  3. Decode MarketEventMessage and convert to Nautilus types")
    else:
        print("\n✗ Some tests failed!")

    sys.exit(0 if all_passed else 1)


if __name__ == "__main__":
    main()
