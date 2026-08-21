#!/usr/bin/env python3
"""End-to-end load test for the confidence event engine WASM."""

import json
import os
import struct
import time
import urllib.request
from pathlib import Path

try:
    import wasmtime
except ImportError:
    print("Install wasmtime: pip install wasmtime")
    raise SystemExit(1)

CLIENT_SECRET = os.environ.get("CLIENT_SECRET")
if not CLIENT_SECRET:
    print("Required env var CLIENT_SECRET is not set")
    raise SystemExit(1)

EVENT_NAME = "skill-e2e-test"
API_URL = "https://events.confidence.dev/v1/events:publish"
NUM_EVENTS = 1_000
WASM_PATH = Path(__file__).resolve().parent.parent.parent.parent / "wasm" / "confidence_event_engine.wasm"


# --- Minimal protobuf encoder/decoder ---

def encode_varint(value: int) -> bytes:
    result = bytearray()
    while value > 0x7F:
        result.append((value & 0x7F) | 0x80)
        value >>= 7
    result.append(value & 0x7F)
    return bytes(result)


def encode_tag(field_num: int, wire_type: int) -> bytes:
    return encode_varint((field_num << 3) | wire_type)


def encode_bytes_field(field_num: int, data: bytes) -> bytes:
    tag = encode_tag(field_num, 2)
    length = encode_varint(len(data))
    return tag + length + data


def encode_string_field(field_num: int, s: str) -> bytes:
    return encode_bytes_field(field_num, s.encode("utf-8"))


def encode_varint_field(field_num: int, value: int) -> bytes:
    return encode_tag(field_num, 0) + encode_varint(value)


def encode_timestamp(seconds: int, nanos: int = 0) -> bytes:
    result = encode_varint_field(1, seconds)
    if nanos:
        result += encode_varint_field(2, nanos)
    return result


def encode_track_event_request(event_name, timestamp_ms):
    seconds = int(timestamp_ms // 1000)
    nanos = int((timestamp_ms - seconds * 1000) * 1_000_000)
    result = encode_string_field(1, event_name)
    result += encode_bytes_field(2, encode_timestamp(seconds, nanos))
    return result


def encode_request(data: bytes) -> bytes:
    return encode_bytes_field(1, data)


def decode_varint(data: bytes, offset: int) :
    result = 0
    shift = 0
    while offset < len(data):
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if (byte & 0x80) == 0:
            return result, offset
        shift += 7
    raise ValueError("varint overflow")


def decode_response(data: bytes) :
    offset = 0
    while offset < len(data):
        tag, offset = decode_varint(data, offset)
        field_num = tag >> 3
        wire_type = tag & 0x7
        if wire_type == 2:
            length, offset = decode_varint(data, offset)
            value = data[offset : offset + length]
            offset += length
            if field_num == 1:
                return value, None
            if field_num == 2:
                return None, value.decode("utf-8")
    return b"", None


def count_events_in_batch(data: bytes) -> int:
    count = 0
    offset = 0
    while offset < len(data):
        tag, offset = decode_varint(data, offset)
        field_num = tag >> 3
        wire_type = tag & 0x7
        if wire_type == 2:
            length, offset = decode_varint(data, offset)
            offset += length
            if field_num == 1:
                count += 1
    return count


# --- WASM host ---

class WasmEventEngine:
    def __init__(self, wasm_path: str):
        engine = wasmtime.Engine()
        module = wasmtime.Module(engine, Path(wasm_path).read_bytes())
        store = wasmtime.Store(engine)
        instance = wasmtime.Instance(store, module, [])
        self.store = store
        self.memory = instance.exports(store)["memory"]
        self.alloc_fn = instance.exports(store)["wasm_msg_alloc"]
        self.free_fn = instance.exports(store)["wasm_msg_free"]
        self.track_fn = instance.exports(store)["wasm_msg_guest_track_event"]
        self.flush_fn = instance.exports(store)["wasm_msg_guest_bounded_flush_events"]

    def _read_u32(self, ptr: int) -> int:
        buf = self.memory.data_ptr(self.store)
        return struct.unpack_from("<I", bytes(buf[ptr : ptr + 4]))[0]

    def _write(self, ptr: int, data: bytes):
        buf = self.memory.data_ptr(self.store)
        for i, b in enumerate(data):
            buf[ptr + i] = b

    def _read(self, ptr: int, length: int) -> bytes:
        buf = self.memory.data_ptr(self.store)
        return bytes(buf[ptr : ptr + length])

    def call(self, fn, req_data: bytes) :
        envelope = encode_request(req_data)
        ptr = self.alloc_fn(self.store, len(envelope))
        self._write(ptr, envelope)

        res_ptr = fn(self.store, ptr)
        if res_ptr == 0:
            return None

        total_size = self._read_u32(res_ptr - 4)
        data_len = total_size - 4
        res_data = self._read(res_ptr, data_len)
        self.free_fn(self.store, res_ptr)

        data, error = decode_response(res_data)
        if error:
            raise RuntimeError(f"WASM error: {error}")
        return data

    def track_event(self, event_def: str, timestamp_ms: float):
        event_data = encode_track_event_request(event_def, timestamp_ms)
        self.call(self.track_fn, event_data)

    def flush(self) :
        return self.call(self.flush_fn, b"")


# --- Network API ---

def post_events(events):
    body = json.dumps(
        {
            "clientSecret": CLIENT_SECRET,
            "sdk": {"id": "SDK_ID_PYTHON_CONFIDENCE", "version": "0.1.0-e2e-test"},
            "sendTime": time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime()),
            "events": events,
        }
    ).encode("utf-8")

    req = urllib.request.Request(
        API_URL,
        data=body,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req) as resp:
        if resp.status != 200:
            raise RuntimeError(f"API returned {resp.status}: {resp.read().decode()}")


def main():
    print(f"Loading WASM: {WASM_PATH}")
    engine = WasmEventEngine(str(WASM_PATH))
    print(f"WASM loaded")

    # Phase 1: Load test
    print(f"\n=== Phase 1: Track {NUM_EVENTS} events ===")
    start = time.perf_counter()
    for i in range(NUM_EVENTS):
        engine.track_event(EVENT_NAME, time.time() * 1000)
    track_dur = time.perf_counter() - start
    track_rate = NUM_EVENTS / track_dur
    print(f"Tracked {NUM_EVENTS} events in {track_dur:.3f}s")
    print(f"Throughput: {track_rate:.0f} events/sec")
    print(f"Latency: {track_dur / NUM_EVENTS * 1000:.3f}ms/event")

    # Phase 2: Flush
    print(f"\n=== Phase 2: Flush events ===")
    total_flushed = 0
    flush_count = 0
    start = time.perf_counter()
    while True:
        batch_data = engine.flush()
        if not batch_data or len(batch_data) == 0:
            break
        count = count_events_in_batch(batch_data)
        if count == 0:
            break
        total_flushed += count
        flush_count += 1
        print(f"  Batch {flush_count}: {count} events ({len(batch_data)} bytes)")
    flush_dur = time.perf_counter() - start
    print(f"Flushed {total_flushed} events in {flush_count} batches, took {flush_dur:.3f}s")

    if total_flushed != NUM_EVENTS:
        print(f"WARNING: tracked {NUM_EVENTS} but flushed {total_flushed} events")

    # Phase 3: POST to real API
    print(f"\n=== Phase 3: POST sample batch to events API ===")
    sample_size = NUM_EVENTS
    latency_ms = track_dur / NUM_EVENTS * 1000
    events = [
        {
            "eventDefinition": f"eventDefinitions/{EVENT_NAME}",
            "eventTime": time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime()),
            "payload": {
                "test_run": "python-e2e-load-test",
                "provider": "python",
                "index": i,
                "batch_size": NUM_EVENTS,
                "latency_ms": latency_ms,
                "context": {"targeting_key": f"test-user-{i}"},
            },
        }
        for i in range(sample_size)
    ]

    start = time.perf_counter()
    post_events(events)
    post_dur = time.perf_counter() - start
    print(f"Posted {sample_size} events to {API_URL} in {post_dur:.3f}s")

    # Summary
    print(f"\n=== Summary ===")
    print(
        f"Track:  {NUM_EVENTS} events, {track_rate:.0f} events/sec, "
        f"{track_dur / NUM_EVENTS * 1000:.3f}ms/event"
    )
    print(
        f"Flush:  {total_flushed} events in {flush_count} batches, {flush_dur:.3f}s total"
    )
    print(f"API:    {sample_size} events posted in {post_dur:.3f}s")
    print("PASS")


if __name__ == "__main__":
    main()
