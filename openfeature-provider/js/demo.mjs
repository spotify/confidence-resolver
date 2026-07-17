#!/usr/bin/env node
/**
 * Demo: run the local provider, resolve flags, and inspect telemetry.
 *
 * Usage:
 *   node demo.mjs --secret <client-secret> [--encryption-key <hex>] [--flag <name>] [--duration <seconds>]
 *
 * Examples:
 *   node demo.mjs --secret abc123 --flag web-sdk-e2e-flag --duration 60
 *   node demo.mjs --secret abc123 --encryption-key deadbeef... --flag web-sdk-e2e-flag --duration 300
 */

import { OpenFeature } from '@openfeature/server-sdk';
import { createConfidenceServerProvider } from './dist/index.node.js';

function parseArgs() {
  const args = process.argv.slice(2);
  const opts = { flags: [], duration: 60 };
  for (let i = 0; i < args.length; i++) {
    switch (args[i]) {
      case '--secret':
        opts.secret = args[++i];
        break;
      case '--encryption-key':
        opts.encryptionKey = args[++i];
        break;
      case '--flag':
        opts.flags.push(args[++i]);
        break;
      case '--duration':
        opts.duration = parseInt(args[++i], 10);
        break;
    }
  }
  if (!opts.secret) {
    console.error('Usage: node demo.mjs --secret <secret> [--encryption-key <key>] [--flag <name>] [--duration <sec>]');
    process.exit(1);
  }
  if (opts.flags.length === 0) opts.flags.push('web-sdk-e2e-flag');
  return opts;
}

const opts = parseArgs();

console.log(`=== Confidence Provider Demo ===`);
console.log(`  encryption: ${opts.encryptionKey ? 'enabled' : 'disabled'}`);
console.log(`  flags:      ${opts.flags.join(', ')}`);
console.log(`  duration:   ${opts.duration}s`);
console.log();

// Minimal protobuf varint + length-delimited decoder for TelemetryData inspection
function decodeProviderInitRate(buf) {
  // Walk WriteFlagLogsRequest looking for field 2 (telemetry_data),
  // then inside that look for field 9 (provider_init_rate)
  let pos = 0;
  while (pos < buf.length) {
    const tag = buf[pos++];
    const fieldNum = tag >> 3;
    const wireType = tag & 0x7;
    if (wireType === 2) {
      // length-delimited
      let len = 0,
        shift = 0;
      while (buf[pos] & 0x80) {
        len |= (buf[pos++] & 0x7f) << shift;
        shift += 7;
      }
      len |= buf[pos++] << shift;
      if (fieldNum === 2) {
        // telemetry_data — recurse into it
        const inner = buf.subarray(pos, pos + len);
        const result = findInitRate(inner);
        if (result) return result;
      }
      pos += len;
    } else if (wireType === 0) {
      // varint
      while (buf[pos++] & 0x80) {}
    } else {
      break;
    }
  }
  return null;
}

function findInitRate(buf) {
  let pos = 0;
  while (pos < buf.length) {
    const tag = buf[pos++];
    const fieldNum = tag >> 3;
    const wireType = tag & 0x7;
    if (wireType === 2) {
      let len = 0,
        shift = 0;
      while (buf[pos] & 0x80) {
        len |= (buf[pos++] & 0x7f) << shift;
        shift += 7;
      }
      len |= buf[pos++] << shift;
      if (fieldNum === 9) {
        // provider_init_rate
        return parseInitRate(buf.subarray(pos, pos + len));
      }
      pos += len;
    } else if (wireType === 0) {
      while (buf[pos++] & 0x80) {}
    } else {
      break;
    }
  }
  return null;
}

function parseInitRate(buf) {
  let count = 0;
  const labels = {};
  let pos = 0;
  while (pos < buf.length) {
    const tag = buf[pos++];
    const fieldNum = tag >> 3;
    const wireType = tag & 0x7;
    if (wireType === 0 && fieldNum === 1) {
      // count
      count = 0;
      let shift = 0;
      while (buf[pos] & 0x80) {
        count |= (buf[pos++] & 0x7f) << shift;
        shift += 7;
      }
      count |= buf[pos++] << shift;
    } else if (wireType === 2 && fieldNum === 3) {
      // labels map entry
      let len = 0,
        shift = 0;
      while (buf[pos] & 0x80) {
        len |= (buf[pos++] & 0x7f) << shift;
        shift += 7;
      }
      len |= buf[pos++] << shift;
      const entry = parseMapEntry(buf.subarray(pos, pos + len));
      if (entry) labels[entry.key] = entry.value;
      pos += len;
    } else if (wireType === 2) {
      let len = 0,
        shift = 0;
      while (buf[pos] & 0x80) {
        len |= (buf[pos++] & 0x7f) << shift;
        shift += 7;
      }
      len |= buf[pos++] << shift;
      pos += len;
    } else if (wireType === 0) {
      while (buf[pos++] & 0x80) {}
    } else {
      break;
    }
  }
  return { count, labels };
}

function parseMapEntry(buf) {
  let key = '',
    value = '';
  let pos = 0;
  while (pos < buf.length) {
    const tag = buf[pos++];
    const fieldNum = tag >> 3;
    const wireType = tag & 0x7;
    if (wireType === 2) {
      let len = 0,
        shift = 0;
      while (buf[pos] & 0x80) {
        len |= (buf[pos++] & 0x7f) << shift;
        shift += 7;
      }
      len |= buf[pos++] << shift;
      const str = new TextDecoder().decode(buf.subarray(pos, pos + len));
      if (fieldNum === 1) key = str;
      else if (fieldNum === 2) value = str;
      pos += len;
    } else {
      break;
    }
  }
  return { key, value };
}

let flushCount = 0;

// Intercept outgoing fetches to log telemetry
const originalFetch = globalThis.fetch;
const wrappedFetch = async (url, init) => {
  let bodyBytes = null;
  if (typeof url === 'string' && url.includes('clientFlagLogs') && init?.body) {
    bodyBytes = new Uint8Array(init.body);
  }
  const resp = await originalFetch(url, init);
  if (bodyBytes) {
    flushCount++;
    const initRate = decodeProviderInitRate(bodyBytes);
    if (initRate) {
      console.log(
        `\n[flush #${flushCount}] → ${resp.status} ★ provider_init_rate: count=${
          initRate.count
        }, labels=${JSON.stringify(initRate.labels)}`,
      );
    } else {
      console.log(`\n[flush #${flushCount}] → ${resp.status} (no provider_init_rate)`);
    }
  }
  return resp;
};

const provider = createConfidenceServerProvider({
  flagClientSecret: opts.secret,
  encryptionKey: opts.encryptionKey,
  flushInterval: 10_000,
  fetch: wrappedFetch,
});

console.log('Initializing provider...');
await OpenFeature.setProviderAndWait(provider);
console.log('Provider ready.\n');

const client = OpenFeature.getClient();
const endTime = Date.now() + opts.duration * 1000;
let resolveCount = 0;

const ctx = { targetingKey: 'demo-user', sticky: false };

while (Date.now() < endTime) {
  for (const flag of opts.flags) {
    const result = client.getBooleanDetails(`${flag}.bool`, false, ctx);
    resolveCount++;
  }

  const remaining = Math.ceil((endTime - Date.now()) / 1000);
  process.stdout.write(`\r  resolves: ${resolveCount} | remaining: ${remaining}s   `);

  await new Promise(r => setTimeout(r, 2000));
}

console.log(`\n\nDone. Total resolves: ${resolveCount}`);
console.log('Shutting down (final flush)...');
await OpenFeature.close();
console.log('Bye.');
