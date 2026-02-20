import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { WasmResolver } from './src/WasmResolver.ts';
import { ResolveProcessRequest } from './src/proto/confidence/wasm/wasm_api.ts';
import { WriteFlagLogsRequest, TelemetryData_BucketSpan, TelemetryData } from './src/proto/test-only.ts';

const moduleBytes = readFileSync(join(import.meta.dirname, '../../wasm/confidence_resolver.wasm'));
const stateBytes = readFileSync(join(import.meta.dirname, '../../wasm/resolver_state.pb'));

const wasmModule = new WebAssembly.Module(moduleBytes);
const CLIENT_SECRET = 'mkjJruAATQWjeY7foFIWfVAcBWnci2YF';

const RESOLVE_REQUEST: ResolveProcessRequest = {
  deferredMaterializations: {
    flags: ['flags/tutorial-feature'],
    clientSecret: CLIENT_SECRET,
    apply: false,
    evaluationContext: {
      targeting_key: 'tutorial_visitor',
      visitor_id: 'tutorial_visitor',
    },
  },
};

// Histogram config from proto annotation on ResolveLatency
const MIN_VALUE = 1;
const MAX_VALUE = 10_000_000;
const BUCKET_COUNT = 162;
const CLOSED_BUCKETS = BUCKET_COUNT - 2;
const LN_MIN = Math.log(MIN_VALUE);
const LN_MAX = Math.log(MAX_VALUE);
const LN_RATIO = (LN_MAX - LN_MIN) / (CLOSED_BUCKETS - 1);

// 10 fixed display buckets with exponential boundaries (microseconds)
// 10 fixed display buckets covering 0–2ms range
const DISPLAY_BUCKETS = [
  { label: '   <1µs', lo: 0, hi: 1 },
  { label: ' 1-5µs', lo: 1, hi: 5 },
  { label: ' 5-10µs', lo: 5, hi: 10 },
  { label: '10-25µs', lo: 10, hi: 25 },
  { label: '25-50µs', lo: 25, hi: 50 },
  { label: ' 50-100µs', lo: 50, hi: 100 },
  { label: '100-250µs', lo: 100, hi: 250 },
  { label: '250-500µs', lo: 250, hi: 500 },
  { label: '0.5-2ms', lo: 500, hi: 2_000 },
  { label: '   >2ms', lo: 2_000, hi: Infinity },
];
const LABEL_WIDTH = Math.max(...DISPLAY_BUCKETS.map((b) => b.label.length));

/** Convert a bucket exponent to its lower boundary in microseconds. */
function bucketLowerBound(exponent: number): number {
  return Math.exp(exponent * LN_RATIO);
}

/** Format microseconds into a human-readable string. */
function formatMicros(us: number): string {
  if (us < 1) return '<1µs';
  if (us < 1000) return `${Math.round(us)}µs`;
  if (us < 1_000_000) return `${(us / 1000).toFixed(1)}ms`;
  return `${(us / 1_000_000).toFixed(2)}s`;
}

/** Expand bucket spans into a Map<exponent, count>. */
function expandBuckets(spans: TelemetryData_BucketSpan[]): Map<number, number> {
  const result = new Map<number, number>();
  for (const span of spans) {
    for (let i = 0; i < span.counts.length; i++) {
      if (span.counts[i] > 0) {
        result.set(span.offset + i, span.counts[i]);
      }
    }
  }
  return result;
}

/** Rebin fine-grained exponent buckets into fixed display buckets. */
function rebinToFixed(fine: Map<number, number>): number[] {
  const counts = new Array(DISPLAY_BUCKETS.length).fill(0);
  for (const [exp, count] of fine) {
    const us = bucketLowerBound(exp);
    const idx = DISPLAY_BUCKETS.findIndex((b) => us < b.hi);
    counts[idx === -1 ? counts.length - 1 : idx] += count;
  }
  return counts;
}

interface Snapshot {
  telemetry: TelemetryData;
  timestamp: number;
  totalResolves: number;
}

/** Find the snapshot closest to 1 second ago from the ring buffer. */
function getOneSecAgo(ring: Snapshot[], now: number): Snapshot | null {
  const target = now - 1000;
  let best: Snapshot | null = null;
  let bestDist = Infinity;
  for (const s of ring) {
    const dist = Math.abs(s.timestamp - target);
    if (dist < bestDist) {
      bestDist = dist;
      best = s;
    }
  }
  return best;
}

/** Estimate a percentile (0–1) from fine-grained delta buckets. */
function estimatePercentile(deltaFine: Map<number, number>, p: number): number {
  const sorted = [...deltaFine.entries()].sort((a, b) => a[0] - b[0]);
  const total = sorted.reduce((sum, [, c]) => sum + c, 0);
  if (total === 0) return 0;
  const target = total * p;
  let cumulative = 0;
  for (const [exp, count] of sorted) {
    cumulative += count;
    if (cumulative >= target) {
      // Interpolate within this bucket
      const lo = bucketLowerBound(exp);
      const hi = bucketLowerBound(exp + 1);
      const fraction = (target - (cumulative - count)) / count;
      return lo + (hi - lo) * fraction;
    }
  }
  return bucketLowerBound(sorted[sorted.length - 1][0]);
}

/** Render the telemetry delta as ASCII art. */
function render(curr: Snapshot, prev: Snapshot | null): string {
  const telemetry = curr.telemetry;
  const dt = prev ? (curr.timestamp - prev.timestamp) / 1000 : 0;
  const lines: string[] = [];

  lines.push('\x1b[1m╔══════════════════════════════════════════════════════════════════════╗\x1b[0m');
  lines.push('\x1b[1m║              RESOLVE LATENCY HISTOGRAM           \x1b[33m— last ~1s\x1b[0m\x1b[1m       ║\x1b[0m');
  lines.push('\x1b[1m╚══════════════════════════════════════════════════════════════════════╝\x1b[0m');

  const latency = telemetry.resolveLatency;
  const prevLatency = prev?.telemetry.resolveLatency;

  if (latency && latency.buckets.length > 0) {
    const currFine = expandBuckets(latency.buckets);
    const prevFine = prevLatency ? expandBuckets(prevLatency.buckets) : new Map<number, number>();

    // Diff fine buckets, then rebin
    const deltaFine = new Map<number, number>();
    for (const [exp, count] of currFine) {
      const delta = count - (prevFine.get(exp) ?? 0);
      if (delta > 0) deltaFine.set(exp, delta);
    }
    const displayCounts = rebinToFixed(deltaFine);
    const maxLog = Math.max(...displayCounts.map((c) => (c > 0 ? Math.log10(c) : 0)), 1);
    const BAR_WIDTH = 44;

    for (let i = 0; i < DISPLAY_BUCKETS.length; i++) {
      const count = displayCounts[i];
      const label = DISPLAY_BUCKETS[i].label.padStart(LABEL_WIDTH);
      const logVal = count > 0 ? Math.log10(count) : 0;
      const barLen = count > 0 ? Math.max(1, Math.ceil((logVal / maxLog) * BAR_WIDTH)) : 0;
      const bar = barLen > 0 ? '\x1b[36m' + '█'.repeat(barLen) + '\x1b[0m' : '';
      const countStr = count > 0 ? ` ${count}` : '';
      lines.push(`  ${label} │${bar}${countStr}`);
    }

    const deltaCount = latency.count - (prevLatency?.count ?? 0);
    const deltaSum = latency.sum - (prevLatency?.sum ?? 0);
    const avg = deltaCount > 0 ? deltaSum / deltaCount : 0;
    const p95 = estimatePercentile(deltaFine, 0.95);
    const rate = dt > 0 ? Math.round(deltaCount / dt) : 0;
    lines.push('');
    lines.push(
      `  \x1b[2mlast ~1s:\x1b[0m ${deltaCount} resolves   avg: ${formatMicros(avg)}   p95: ${formatMicros(p95)}   ${rate}/s`,
    );
    lines.push(
      `  \x1b[2m   total:\x1b[0m ${latency.count} resolves   avg: ${formatMicros(latency.count > 0 ? latency.sum / latency.count : 0)}`,
    );
  } else {
    lines.push('  (no latency data)');
  }

  lines.push('');
  lines.push(`  total resolves: ${curr.totalResolves}`);

  return lines.join('\n');
}

// --- Main ---

const resolver = new WasmResolver(wasmModule);
resolver.setResolverState({ state: stateBytes, accountId: 'confidence-test' });

const RENDER_INTERVAL_MS = 200;
const RING_SIZE = Math.ceil(1000 / RENDER_INTERVAL_MS) + 1; // ~1 second of history
let totalResolves = 0;
let running = true;
const ring: Snapshot[] = [];

// Resolve as fast as possible in a busy loop on the main thread,
// yielding to the event loop via setImmediate so the render timer fires.
function resolveLoop() {
  const deadline = Date.now() + 10; // work for up to 10ms then yield
  while (Date.now() < deadline) {
    resolver.resolveProcess(RESOLVE_REQUEST);
    totalResolves++;
  }
  if (running) setImmediate(resolveLoop);
}

function render_tick() {
  const decoded = WriteFlagLogsRequest.decode(resolver.flushLogs());
  if (!decoded.telemetryData) return;

  const now = Date.now();
  const curr: Snapshot = {
    telemetry: decoded.telemetryData,
    timestamp: now,
    totalResolves,
  };

  const prev = getOneSecAgo(ring, now);

  // Clear screen and render
  process.stdout.write('\x1b[2J\x1b[H');
  process.stdout.write(render(curr, prev) + '\n');

  ring.push(curr);
  if (ring.length > RING_SIZE) ring.shift();
}

// Run until interrupted
console.log('Starting telemetry demo... (Ctrl+C to stop)');
const interval = setInterval(render_tick, RENDER_INTERVAL_MS);
setImmediate(resolveLoop);

process.on('SIGINT', () => {
  clearInterval(interval);
  console.log('\nDone.');
  process.exit(0);
});
