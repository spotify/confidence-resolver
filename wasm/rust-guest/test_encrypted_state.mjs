#!/usr/bin/env node
// WASM-level test for set_encrypted_resolver_state.
// Loads the compiled WASM binary, feeds it the encrypted test fixture,
// and verifies it decrypts and sets state successfully.

import { readFileSync } from 'fs';

const wasmPath = new URL('../confidence_resolver.wasm', import.meta.url).pathname;
const encryptedPath = new URL('../../data/resolver_state_encrypted.pb', import.meta.url).pathname;
const keyPath = new URL('../../data/encryption_key_test.hex', import.meta.url).pathname;

const wasmBuffer = readFileSync(wasmPath);
const encryptedState = readFileSync(encryptedPath);
const encryptionKey = Buffer.from(readFileSync(keyPath, 'utf8').trim(), 'hex');

// Minimal protobuf encoder
function encodeVarint(n) {
  const bytes = [];
  while (n > 0x7f) { bytes.push((n & 0x7f) | 0x80); n >>>= 7; }
  bytes.push(n & 0x7f);
  return Buffer.from(bytes);
}

function encodeField(fieldNum, wireType, data) {
  const tag = encodeVarint((fieldNum << 3) | wireType);
  return Buffer.concat([tag, encodeVarint(data.length), data]);
}

function buildEncryptedRequest(encrypted, key) {
  return encodeField(1, 2, Buffer.concat([
    encodeField(1, 2, encrypted),
    encodeField(2, 2, key),
  ]));
}

function parseResponse(buf) {
  let offset = 0;
  while (offset < buf.length) {
    const byte = buf[offset++];
    const fieldNum = byte >> 3;
    const wireType = byte & 0x7;
    if (wireType === 2) {
      let len = 0, shift = 0, b;
      do { b = buf[offset++]; len |= (b & 0x7f) << shift; shift += 7; } while (b & 0x80);
      const val = buf.subarray(offset, offset + len);
      if (fieldNum === 2) return { error: val.toString() };
      if (fieldNum === 1) return { data: val };
      offset += len;
    } else if (wireType === 0) {
      while (buf[offset++] & 0x80) {}
    }
  }
  return {};
}

let inst;

function hostCurrentTime(ptr) {
  inst.exports.wasm_msg_free(ptr);
  const secs = Math.floor(Date.now() / 1000);
  const ts = Buffer.concat([
    encodeVarint((1 << 3) | 0), encodeVarint(secs),
    encodeVarint((2 << 3) | 0), encodeVarint(0),
  ]);
  const resp = encodeField(1, 2, ts);
  const p = inst.exports.wasm_msg_alloc(resp.length);
  new Uint8Array(inst.exports.memory.buffer, p, resp.length).set(resp);
  return p;
}

function callWasm(fnName, requestBytes) {
  const ptr = inst.exports.wasm_msg_alloc(requestBytes.length);
  new Uint8Array(inst.exports.memory.buffer, ptr, requestBytes.length).set(requestBytes);
  const resPtr = inst.exports[fnName](ptr);
  const view = new DataView(inst.exports.memory.buffer);
  const size = view.getUint32(resPtr - 4, true) - 4;
  const resBytes = Buffer.from(new Uint8Array(inst.exports.memory.buffer, resPtr, size));
  inst.exports.wasm_msg_free(resPtr);
  return parseResponse(resBytes);
}

const mod = new WebAssembly.Module(wasmBuffer);
inst = new WebAssembly.Instance(mod, {
  wasm_msg: { wasm_msg_host_current_time: hostCurrentTime },
});

let passed = 0;
let failed = 0;

function test(name, fn) {
  try {
    fn();
    console.log(`  ✓ ${name}`);
    passed++;
  } catch (e) {
    console.log(`  ✗ ${name}: ${e.message}`);
    failed++;
  }
}

function assert(condition, msg) {
  if (!condition) throw new Error(msg || 'assertion failed');
}

console.log('set_encrypted_resolver_state');

test('decrypts and sets state successfully', () => {
  const req = buildEncryptedRequest(encryptedState, encryptionKey);
  const res = callWasm('wasm_msg_guest_set_encrypted_resolver_state', req);
  assert(!res.error, `unexpected error: ${res.error}`);
});

test('rejects wrong encryption key', () => {
  const wrongKey = Buffer.alloc(32, 0xff);
  const req = buildEncryptedRequest(encryptedState, wrongKey);
  const res = callWasm('wasm_msg_guest_set_encrypted_resolver_state', req);
  assert(res.error, 'expected error for wrong key');
  assert(res.error.includes('decrypt'), `expected decrypt error, got: ${res.error}`);
});

test('rejects invalid key length', () => {
  const shortKey = Buffer.alloc(16);
  const req = buildEncryptedRequest(encryptedState, shortKey);
  const res = callWasm('wasm_msg_guest_set_encrypted_resolver_state', req);
  assert(res.error, 'expected error for short key');
  assert(res.error.includes('32 bytes'), `expected key length error, got: ${res.error}`);
});

console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed > 0 ? 1 : 0);
