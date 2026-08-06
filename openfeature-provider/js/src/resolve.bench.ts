import { bench, describe } from 'vitest';
import { StateModule } from './StateModule';
import { readFileSync } from 'node:fs';
import { ResolveFlagsRequest, EvaluateRequest } from './proto/state_module/api';

// state generated from state-to-wasm fuzz/src/main/resources/benchmark-state.json
const stateModuleBytes = readFileSync(__dirname + '/../../../wasm/benchmark_state.wasm');

const CLIENT_SECRET = 'bench-secret';

const EVALUATION_CONTEXT = {
  targeting_key: 'user-42',
  country: 'SE',
  age: 25,
};

// active flags in benchmark-state.json
const FLAGS = [
  'flags/catch-all',
  'flags/targeted-eq',
  'flags/multi-variant',
  'flags/multi-rule',
  'flags/and-segment',
  'flags/nested-segment',
  'flags/materialized', // suspends on deferred resolve (reads materializations/mat1)
  'flags/cse-heavy',
];

function resolveRequest(flags: string[]): ResolveFlagsRequest {
  return { flags, clientSecret: CLIENT_SECRET, apply: false, evaluationContext: EVALUATION_CONTEXT };
}

const SINGLE_FLAG_RESOLVES = FLAGS.map(flag => resolveRequest([flag]));
const ALL_FLAGS_RESOLVE = resolveRequest([]);

const stateModule = new StateModule(new WebAssembly.Module(stateModuleBytes));

// Resolve with deferred materializations, resuming immediately on suspend.
// Resuming with no records is the "miss" path: no stored assignment found,
// so the resolver assigns fresh and returns materializations to write.
function resolveDeferred(module: StateModule, request: ResolveFlagsRequest): void {
  const started = module.resolveStart(request);
  try {
    const suspended = started.decode().suspended;
    if (suspended) module.resolveResume(suspended.processId, []).close();
  } finally {
    started.close();
  }
}

// each bench gets its own index so every run sees the same request sequence
function cycle<T>(items: T[], run: (item: T) => void) {
  let index = 0;
  return () => run(items[index++ % items.length]);
}

describe('singleFlag', () => {
  bench(
    'resolve',
    cycle(SINGLE_FLAG_RESOLVES, r => resolveDeferred(stateModule, r)),
    { warmupIterations: 50_000 },
  );
});

describe('allFlags', () => {
  bench('resolve', () => resolveDeferred(stateModule, ALL_FLAGS_RESOLVE), { warmupIterations: 10_000 });
});

// resolve once, then evaluate against the borrowed response — the provider's
// hot path, and the one the host used to walk in JS
describe('evaluate', () => {
  const handle = stateModule.resolve(resolveRequest(['flags/catch-all']));
  const request: EvaluateRequest = { flagKey: 'catch-all', defaultValue: null };
  bench('evaluate against a resolved response', () => void handle.evaluate(request), { warmupIterations: 50_000 });
});
