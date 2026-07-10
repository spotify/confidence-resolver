import { bench, describe } from 'vitest';
import { WasmResolver, UnsafeWasmResolver } from './WasmResolver';
import { WasmStateResolver } from './WasmStateResolver';
import { StateModule } from './StateModule';
import { readFileSync } from 'node:fs';
import type { LocalResolver } from './LocalResolver';
import { ResolveProcessRequest, ResolveProcessResponse } from './proto/confidence/wasm/wasm_api';
import { Request } from './proto/confidence/wasm/messages';
import {
  Materialization,
  ResolveFlagsRequest as SmResolveFlagsRequest,
  type ResolveFlagsResponse as SmResolveFlagsResponse,
} from './proto/state_module/api';

// state generated from state-to-wasm fuzz/src/main/resources/benchmark-state.json
const moduleBytes = readFileSync(__dirname + '/../../../wasm/confidence_resolver.wasm');
const stateBytes = readFileSync(__dirname + '/../../../wasm/benchmark_state.pb');
const stateModuleBytes = readFileSync(__dirname + '/../../../wasm/benchmark_state.wasm');

const module = new WebAssembly.Module(moduleBytes);
const CLIENT_SECRET = 'bench-secret';
const ACCOUNT_ID = 'bench-account';

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

function resolveRequest(flags: string[]): SmResolveFlagsRequest {
  return {
    flags,
    clientSecret: CLIENT_SECRET,
    apply: false,
    evaluationContext: EVALUATION_CONTEXT,
  };
}

const SINGLE_FLAG_RESOLVES = FLAGS.map(flag => resolveRequest([flag]));
const ALL_FLAGS_RESOLVE = resolveRequest([]);

const SINGLE_FLAG_REQUESTS: ResolveProcessRequest[] = SINGLE_FLAG_RESOLVES.map(deferredMaterializations => ({
  deferredMaterializations,
}));
const ALL_FLAGS_REQUEST: ResolveProcessRequest = { deferredMaterializations: ALL_FLAGS_RESOLVE };

const wasmResolver = new WasmResolver(module);
wasmResolver.setResolverState({ state: stateBytes, accountId: ACCOUNT_ID });

const wasmStateResolver = new WasmStateResolver();
wasmStateResolver.setResolverState({ state: stateModuleBytes, accountId: ACCOUNT_ID });

const stateModule = new StateModule(new WebAssembly.Module(stateModuleBytes));

// Resolve with deferred materializations, resuming immediately on suspend.
// Resuming with no records is the "miss" path: no stored assignment found,
// so the resolver assigns fresh and returns materializations to write.
function resolveDeferred(resolver: LocalResolver, request: ResolveProcessRequest): ResolveProcessResponse {
  const response = resolver.resolveProcess(request);
  if (response.suspended) {
    return resolver.resolveProcess({
      resume: { materializations: [], state: response.suspended.state },
    });
  }
  return response;
}

// same flow through the native StateModule interface
function resolveDeferredNative(module: StateModule, request: SmResolveFlagsRequest): SmResolveFlagsResponse {
  const response = module.resolveProcessStart(request);
  if (response.suspended) {
    return module.resolveProcessResume(response.suspended.processId, []);
  }
  return response.resolved!;
}

// each bench gets its own index so all resolvers see the same request sequence
function singleFlagBench(resolver: LocalResolver) {
  let index = 0;
  return () => {
    resolveDeferred(resolver, SINGLE_FLAG_REQUESTS[index++ % SINGLE_FLAG_REQUESTS.length]);
  };
}

function singleFlagBenchNative(module: StateModule) {
  let index = 0;
  return () => {
    resolveDeferredNative(module, SINGLE_FLAG_RESOLVES[index++ % SINGLE_FLAG_RESOLVES.length]);
  };
}

describe('singleFlag', () => {
  bench('WasmResolver (current)', singleFlagBench(wasmResolver), { warmupIterations: 50_000 });
  bench('WasmStateResolver (adapter)', singleFlagBench(wasmStateResolver), { warmupIterations: 50_000 });
  bench('StateModule (native)', singleFlagBenchNative(stateModule), { warmupIterations: 50_000 });
});

describe('allFlags', () => {
  bench(
    'WasmResolver (current)',
    () => {
      resolveDeferred(wasmResolver, ALL_FLAGS_REQUEST);
    },
    { warmupIterations: 10_000 },
  );
  bench(
    'WasmStateResolver (adapter)',
    () => {
      resolveDeferred(wasmStateResolver, ALL_FLAGS_REQUEST);
    },
    { warmupIterations: 10_000 },
  );
  bench(
    'StateModule (native)',
    () => {
      resolveDeferredNative(stateModule, ALL_FLAGS_RESOLVE);
    },
    { warmupIterations: 10_000 },
  );
});

// --- raw benches: everything except proto encode/decode ---
//
// Requests are pre-encoded and responses are copied out but never decoded, so
// what remains is memory transfer + the wasm call itself. Since control flow
// can't branch on an undecoded response, both sides use the static (empty)
// materializations path, which never suspends: flags/materialized finds no
// stored assignment and assigns fresh, like the miss path above but without
// the suspend/resume round trip.

const unsafeResolver = new UnsafeWasmResolver(module);
unsafeResolver.setResolverState({ state: stateBytes, accountId: ACCOUNT_ID });
// reach into the private exports to drive the wasm-msg protocol directly
const oldExports = (unsafeResolver as unknown as { exports: any }).exports as {
  memory: WebAssembly.Memory;
  wasm_msg_alloc(size: number): number;
  wasm_msg_free(ptr: number): void;
  wasm_msg_guest_resolve_flags(ptr: number): number;
};

interface RawStateModuleExports {
  memory: WebAssembly.Memory;
  alloc(size: number): number;
  free(ptr: number): void;
  resolve_flags(request: bigint, materializations: bigint): bigint;
}
const rawExports: RawStateModuleExports = new WebAssembly.Instance(new WebAssembly.Module(stateModuleBytes), {
  env: {
    current_time: () => BigInt(Date.now()),
    // never called with apply: false and no flushes, but free defensively
    write_logs: (payload: bigint) => rawExports.free(Number(payload & 0xffffffffn)),
  },
}).exports as unknown as RawStateModuleExports;

function encodeOldRequest(resolve: SmResolveFlagsRequest): Uint8Array {
  const data = ResolveProcessRequest.encode({
    staticMaterializations: { resolveRequest: resolve, materializations: [] },
  }).finish();
  return Request.encode({ data }).finish();
}

const OLD_RAW_SINGLE = SINGLE_FLAG_RESOLVES.map(encodeOldRequest);
const OLD_RAW_ALL = encodeOldRequest(ALL_FLAGS_RESOLVE);
const SM_RAW_SINGLE = SINGLE_FLAG_RESOLVES.map(resolve => SmResolveFlagsRequest.encode(resolve).finish());
const SM_RAW_ALL = SmResolveFlagsRequest.encode(ALL_FLAGS_RESOLVE).finish();
const EMPTY_MATERIALIZATIONS = Materialization.encode({ records: [] }).finish();

function oldRawResolve(reqBytes: Uint8Array): Uint8Array {
  const reqPtr = oldExports.wasm_msg_alloc(reqBytes.length);
  new Uint8Array(oldExports.memory.buffer, reqPtr, reqBytes.length).set(reqBytes);
  const resPtr = oldExports.wasm_msg_guest_resolve_flags(reqPtr);
  const size = new DataView(oldExports.memory.buffer).getUint32(resPtr - 4, true);
  const response = new Uint8Array(oldExports.memory.buffer, resPtr, size - 4).slice();
  oldExports.wasm_msg_free(resPtr);
  return response;
}

function smRawResolve(reqBytes: Uint8Array): Uint8Array {
  const reqPtr = rawExports.alloc(reqBytes.length);
  new Uint8Array(rawExports.memory.buffer, reqPtr, reqBytes.length).set(reqBytes);
  const matPtr = rawExports.alloc(EMPTY_MATERIALIZATIONS.length);
  new Uint8Array(rawExports.memory.buffer, matPtr, EMPTY_MATERIALIZATIONS.length).set(EMPTY_MATERIALIZATIONS);
  const resSlice = rawExports.resolve_flags(
    (BigInt(reqBytes.length) << 32n) | BigInt(reqPtr),
    (BigInt(EMPTY_MATERIALIZATIONS.length) << 32n) | BigInt(matPtr),
  );
  const resPtr = Number(resSlice & 0xffffffffn);
  const resLen = Number((resSlice >> 32n) & 0xffffffffn);
  const response = new Uint8Array(rawExports.memory.buffer, resPtr, resLen).slice();
  rawExports.free(resPtr);
  return response;
}

// the raw benches never decode responses, so errors (which travel inside the
// response bytes) would silently bench an error path — verify each request
// resolves cleanly through the decoding APIs on the same instances first
for (const resolve of [...SINGLE_FLAG_RESOLVES, ALL_FLAGS_RESOLVE]) {
  const oldResponse = unsafeResolver.resolveProcess({
    staticMaterializations: { resolveRequest: resolve, materializations: [] },
  });
  const smResponse = stateModule.resolveFlags(resolve, []);
  for (const { response, name } of [
    { response: oldResponse.resolved?.response, name: 'old' },
    { response: smResponse, name: 'new' },
  ]) {
    if (!response || response.resolvedFlags.some(flag => flag.reason > 4)) {
      throw new Error(`${name} raw path failed for [${resolve.flags}]: ${JSON.stringify(response)}`);
    }
  }
}

function singleFlagBenchRaw(resolve: (reqBytes: Uint8Array) => Uint8Array, requests: Uint8Array[]) {
  let index = 0;
  return () => {
    resolve(requests[index++ % requests.length]);
  };
}

describe('singleFlag raw', () => {
  bench('WasmResolver (current)', singleFlagBenchRaw(oldRawResolve, OLD_RAW_SINGLE), { warmupIterations: 50_000 });
  bench('StateModule (new)', singleFlagBenchRaw(smRawResolve, SM_RAW_SINGLE), { warmupIterations: 50_000 });
});

describe('allFlags raw', () => {
  bench(
    'WasmResolver (current)',
    () => {
      oldRawResolve(OLD_RAW_ALL);
    },
    { warmupIterations: 10_000 },
  );
  bench(
    'StateModule (new)',
    () => {
      smRawResolve(SM_RAW_ALL);
    },
    { warmupIterations: 10_000 },
  );
});
