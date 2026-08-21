import { readFile } from "node:fs/promises";

const CLIENT_SECRET = process.env.CLIENT_SECRET;
if (!CLIENT_SECRET) {
  console.error("Required env var CLIENT_SECRET is not set");
  process.exit(1);
}
const EVENT_NAME = "skill-e2e-test";
const API_URL = "https://events.confidence.dev/v1/events:publish";
const NUM_EVENTS = 1_000;
const WASM_PATH = new URL(
  "../../../wasm/confidence_event_engine.wasm",
  import.meta.url,
);

// --- Minimal protobuf encoder/decoder (no dependencies) ---

function encodeVarint(value) {
  const bytes = [];
  while (value > 0x7f) {
    bytes.push((value & 0x7f) | 0x80);
    value >>>= 7;
  }
  bytes.push(value & 0x7f);
  return new Uint8Array(bytes);
}

function encodeBigVarint(value) {
  const bytes = [];
  let v = BigInt(value);
  while (v > 0x7fn) {
    bytes.push(Number(v & 0x7fn) | 0x80);
    v >>= 7n;
  }
  bytes.push(Number(v & 0x7fn));
  return new Uint8Array(bytes);
}

function encodeTag(fieldNum, wireType) {
  return encodeVarint((fieldNum << 3) | wireType);
}

function encodeBytes(fieldNum, data) {
  const tag = encodeTag(fieldNum, 2); // wire type 2 = length-delimited
  const len = encodeVarint(data.length);
  const result = new Uint8Array(tag.length + len.length + data.length);
  result.set(tag, 0);
  result.set(len, tag.length);
  result.set(data, tag.length + len.length);
  return result;
}

function encodeString(fieldNum, str) {
  return encodeBytes(fieldNum, new TextEncoder().encode(str));
}

function encodeTimestamp(seconds, nanos) {
  let parts = [encodeBigVarintField(1, BigInt(seconds))];
  if (nanos) parts.push(encodeVarintField(2, nanos));
  return concat(parts);
}

function encodeVarintField(fieldNum, value) {
  const tag = encodeTag(fieldNum, 0);
  const val = encodeVarint(value);
  return concat([tag, val]);
}

function encodeBigVarintField(fieldNum, value) {
  const tag = encodeTag(fieldNum, 0);
  const val = encodeBigVarint(value);
  return concat([tag, val]);
}

function concat(arrays) {
  const totalLen = arrays.reduce((sum, a) => sum + a.length, 0);
  const result = new Uint8Array(totalLen);
  let offset = 0;
  for (const a of arrays) {
    result.set(a, offset);
    offset += a.length;
  }
  return result;
}

function encodeTrackEventRequest(eventName, timestampMs) {
  const seconds = Math.floor(timestampMs / 1000);
  const nanos = Math.round((timestampMs - seconds * 1000) * 1_000_000);
  const ts = encodeTimestamp(seconds, nanos);
  return concat([encodeString(1, eventName), encodeBytes(2, ts)]);
}

function encodeRequest(data) {
  return encodeBytes(1, data);
}

function decodeVarint(data, offset) {
  let result = 0;
  let shift = 0;
  while (offset < data.length) {
    const byte = data[offset++];
    result |= (byte & 0x7f) << shift;
    if ((byte & 0x80) === 0) return [result, offset];
    shift += 7;
  }
  throw new Error("varint overflow");
}

function decodeResponse(data) {
  let offset = 0;
  while (offset < data.length) {
    const [tag, newOff] = decodeVarint(data, offset);
    offset = newOff;
    const fieldNum = tag >> 3;
    const wireType = tag & 0x7;
    if (wireType === 2) {
      const [len, dataOff] = decodeVarint(data, offset);
      const value = data.slice(dataOff, dataOff + len);
      offset = dataOff + len;
      if (fieldNum === 1) return { data: value };
      if (fieldNum === 2)
        return { error: new TextDecoder().decode(value) };
    }
  }
  return { data: new Uint8Array(0) };
}

function countEventsInBatch(data) {
  let count = 0;
  let offset = 0;
  while (offset < data.length) {
    const [tag, newOff] = decodeVarint(data, offset);
    offset = newOff;
    const fieldNum = tag >> 3;
    const wireType = tag & 0x7;
    if (wireType === 2) {
      const [len, dataOff] = decodeVarint(data, offset);
      offset = dataOff + len;
      if (fieldNum === 1) count++;
    }
  }
  return count;
}

// --- WASM host ---

class WasmEventEngine {
  constructor(instance) {
    this.instance = instance;
    this.exports = instance.exports;
  }

  viewBuffer(ptr) {
    const dv = new DataView(this.exports.memory.buffer);
    const totalSize = dv.getUint32(ptr - 4, true);
    return new Uint8Array(this.exports.memory.buffer, ptr, totalSize - 4);
  }

  call(fn, reqData) {
    const envelope = encodeRequest(reqData);
    const ptr = this.exports.wasm_msg_alloc(envelope.length);
    this.viewBuffer(ptr).set(envelope);

    const resPtr = fn(ptr);
    if (resPtr === 0) return null;

    const resBytes = this.viewBuffer(resPtr).slice(); // defensive copy
    this.exports.wasm_msg_free(resPtr);
    return decodeResponse(resBytes);
  }

  trackEvent(eventName, timestampMs) {
    const eventData = encodeTrackEventRequest(eventName, timestampMs);
    const res = this.call(this.exports.wasm_msg_guest_track_event, eventData);
    if (res?.error) throw new Error(res.error);
  }

  flush() {
    const res = this.call(
      this.exports.wasm_msg_guest_bounded_flush_events,
      new Uint8Array(0),
    );
    if (res?.error) throw new Error(res.error);
    return res?.data;
  }
}

// --- Network API ---

async function postEvents(events) {
  const body = JSON.stringify({
    clientSecret: CLIENT_SECRET,
    sdk: { id: "SDK_ID_JS_CONFIDENCE", version: "0.1.0-e2e-test" },
    sendTime: new Date().toISOString(),
    events,
  });

  const resp = await fetch(API_URL, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body,
  });

  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`API returned ${resp.status}: ${text}`);
  }
}

// --- Main ---

async function main() {
  const wasmBytes = await readFile(WASM_PATH);
  console.log(`Loaded WASM: ${wasmBytes.length} bytes`);

  const { instance } = await WebAssembly.instantiate(wasmBytes);
  const engine = new WasmEventEngine(instance);

  // Phase 1: Load test
  console.log(`\n=== Phase 1: Track ${NUM_EVENTS} events ===`);
  const trackStart = performance.now();
  for (let i = 0; i < NUM_EVENTS; i++) {
    engine.trackEvent(EVENT_NAME, Date.now());
  }
  const trackMs = performance.now() - trackStart;
  const trackRate = Math.round(NUM_EVENTS / (trackMs / 1000));
  console.log(`Tracked ${NUM_EVENTS} events in ${trackMs.toFixed(1)}ms`);
  console.log(`Throughput: ${trackRate} events/sec`);
  console.log(
    `Latency: ${(trackMs / NUM_EVENTS).toFixed(3)}ms/event`,
  );

  // Phase 2: Flush
  console.log(`\n=== Phase 2: Flush events ===`);
  let totalFlushed = 0;
  let flushCount = 0;
  const flushStart = performance.now();
  while (true) {
    const batchData = engine.flush();
    if (!batchData || batchData.length === 0) break;
    const count = countEventsInBatch(batchData);
    if (count === 0) break;
    totalFlushed += count;
    flushCount++;
    console.log(
      `  Batch ${flushCount}: ${count} events (${batchData.length} bytes)`,
    );
  }
  const flushMs = performance.now() - flushStart;
  console.log(
    `Flushed ${totalFlushed} events in ${flushCount} batches, took ${flushMs.toFixed(1)}ms`,
  );

  if (totalFlushed !== NUM_EVENTS) {
    console.error(
      `WARNING: tracked ${NUM_EVENTS} but flushed ${totalFlushed} events`,
    );
  }

  // Phase 3: POST to real API
  console.log(`\n=== Phase 3: POST sample batch to events API ===`);
  const sampleSize = NUM_EVENTS;
  const latencyMs = trackMs / NUM_EVENTS;
  const events = Array.from({ length: sampleSize }, (_, i) => ({
    eventDefinition: `eventDefinitions/${EVENT_NAME}`,
    eventTime: new Date().toISOString(),
    payload: {
      test_run: "js-e2e-load-test",
      provider: "js",
      index: i,
      batch_size: NUM_EVENTS,
      latency_ms: latencyMs,
      context: { targeting_key: `test-user-${i}` },
    },
  }));

  const postStart = performance.now();
  await postEvents(events);
  const postMs = performance.now() - postStart;
  console.log(
    `Posted ${sampleSize} events to ${API_URL} in ${postMs.toFixed(1)}ms`,
  );

  // Summary
  console.log(`\n=== Summary ===`);
  console.log(
    `Track:  ${NUM_EVENTS} events, ${trackRate} events/sec, ${(trackMs / NUM_EVENTS).toFixed(3)}ms/event`,
  );
  console.log(
    `Flush:  ${totalFlushed} events in ${flushCount} batches, ${flushMs.toFixed(1)}ms total`,
  );
  console.log(`API:    ${sampleSize} events posted in ${postMs.toFixed(1)}ms`);
  console.log("PASS");
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
