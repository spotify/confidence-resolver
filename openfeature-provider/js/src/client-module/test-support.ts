import { readFileSync } from 'node:fs';
import type { GuestLogPayload, HostFunctions } from './guest-instance';

// --- fixture loading -----------------------------------------------------------

export function loadFixture(name: string): Uint8Array<ArrayBuffer> {
  const bytes = readFileSync(new URL(`./__fixtures__/${name}`, import.meta.url));
  // Copy: Node's Buffer is Uint8Array<ArrayBufferLike>, wasm APIs want ArrayBuffer-backed bytes.
  return new Uint8Array(bytes);
}

// --- recording host functions ---------------------------------------------------

export interface RecordingHost {
  host: HostFunctions;
  randomSeeds: bigint[];
  currentTimes: bigint[];
  writeLogPayloads: Uint8Array[];
  applySeenQueries: bigint[];
  schemaSeenQueries: bigint[];
  hostErrors: Array<{ where: string; error: unknown }>;
  /** Fingerprints the host reports as already seen. */
  applySeen: Set<bigint>;
  schemaSeen: Set<bigint>;
  /** When true, the writeLogs implementation acks the payload itself. */
  writeLogsAcks: boolean;
  /** When true, the writeLogs implementation throws. */
  writeLogsThrows: boolean;
}

/** A `HostFunctions` implementation that records every call. */
export function createRecordingHost(): RecordingHost {
  const recording: RecordingHost = {
    randomSeeds: [],
    currentTimes: [],
    writeLogPayloads: [],
    applySeenQueries: [],
    schemaSeenQueries: [],
    hostErrors: [],
    applySeen: new Set(),
    schemaSeen: new Set(),
    writeLogsAcks: false,
    writeLogsThrows: false,
    host: {
      currentTime: () => {
        const now = 1_700_000_000_000n + BigInt(recording.currentTimes.length);
        recording.currentTimes.push(now);
        return now;
      },
      randomSeed: () => {
        const seed = 1_000n + BigInt(recording.randomSeeds.length);
        recording.randomSeeds.push(seed);
        return seed;
      },
      writeLogs: (payload: GuestLogPayload) => {
        recording.writeLogPayloads.push(payload.bytes());
        if (recording.writeLogsThrows) {
          throw new Error('boom from writeLogs');
        }
        if (recording.writeLogsAcks) {
          payload.ack();
        }
      },
      applyFilterSeen: (fingerprint: bigint) => {
        recording.applySeenQueries.push(fingerprint);
        return recording.applySeen.has(fingerprint);
      },
      schemaFilterSeen: (fingerprint: bigint) => {
        recording.schemaSeenQueries.push(fingerprint);
        return recording.schemaSeen.has(fingerprint);
      },
      onHostFunctionError: (where, error) => {
        recording.hostErrors.push({ where, error });
      },
    },
  };
  return recording;
}

// --- fake guest shared-memory layout --------------------------------------------

/**
 * Observable slots the fake guest keeps in shared memory:
 * [0..4) bump top, [4..8) destroy counter, [8..16) last ack, [16..24) last
 * write_logs payload, [24..32) last random_seed.
 */
export function guestLayout(memory: WebAssembly.Memory) {
  return {
    bumpTop: () => Atomics.load(new Int32Array(memory.buffer), 0),
    destroyCount: () => Atomics.load(new Int32Array(memory.buffer), 1),
    lastAck: () => Atomics.load(new BigInt64Array(memory.buffer), 1),
    lastWriteLogs: () => Atomics.load(new BigInt64Array(memory.buffer), 2),
    lastSeed: () => Atomics.load(new BigInt64Array(memory.buffer), 3),
  };
}

// --- minimal wasm binary builders ------------------------------------------------

const WASM_HEADER = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

function uleb(value: number): number[] {
  const out: number[] = [];
  let v = value >>> 0;
  do {
    let byte = v & 0x7f;
    v >>>= 7;
    if (v !== 0) byte |= 0x80;
    out.push(byte);
  } while (v !== 0);
  return out;
}

function str(s: string): number[] {
  const bytes = [...new TextEncoder().encode(s)];
  return [...uleb(bytes.length), ...bytes];
}

function section(id: number, content: number[]): number[] {
  return [id, ...uleb(content.length), ...content];
}

function wasm(...sections: number[][]): Uint8Array<ArrayBuffer> {
  return new Uint8Array([...WASM_HEADER, ...sections.flat()]);
}

function hostMemoryImport(initial: number, maximum: number): number[] {
  return [...str('host'), ...str('memory'), 0x02, 0b11, ...uleb(initial), ...uleb(maximum)];
}

/**
 * A module that imports a conforming shared host.memory but has no exports:
 * passes `readSharedMemoryLimits`, fails `GuestInstance` export verification.
 */
export function guestModuleWithoutExports(): Uint8Array<ArrayBuffer> {
  return wasm(section(2, [1, ...hostMemoryImport(1, 2)]));
}

/** A module that does not import host.memory at all. */
export function moduleWithoutHostMemory(): Uint8Array<ArrayBuffer> {
  return new Uint8Array(WASM_HEADER);
}
