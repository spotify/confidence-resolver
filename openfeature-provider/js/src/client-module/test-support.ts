import { readFileSync } from 'node:fs';
import type { GuestLogPayload, HostFunctions } from './guest-instance';
import { createCipheriv, createHash } from 'node:crypto';
import {
  ResolveProcessResponse,
  ResolutionDetails as WasmResolutionDetails,
} from '../proto/confidence/flags/wasm/v1/api';
import { ResolverMetadata } from '../proto/confidence/flags/wasm/v1/metadata';
import { ClientModule } from '../proto/confidence/flags/wasm/v1/client_module';
import { ResolveReason } from '../proto/confidence/flags/resolver/v1/types';

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

export function wasm(...sections: number[][]): Uint8Array<ArrayBuffer> {
  return new Uint8Array([...WASM_HEADER, ...sections.flat()]);
}

export function hostMemoryImport(initial: number, maximum: number): number[] {
  return [...str('host'), ...str('memory'), 0x02, 0b11, ...uleb(initial), ...uleb(maximum)];
}

/** A custom section: id 0, content = length-prefixed name + payload. */
export function customSection(name: string, payload: Uint8Array | number[]): number[] {
  const content = [...str(name), ...(payload instanceof Uint8Array ? [...payload] : payload)];
  return section(0, content);
}

/**
 * A module that imports a conforming shared host.memory but has no exports:
 * passes `readSharedMemoryLimits`, fails `GuestInstance` export verification.
 */
export function guestModuleWithoutExports(): Uint8Array<ArrayBuffer> {
  return wasm(section(2, [1, ...hostMemoryImport(1, 2)]));
}

/** An import section with the given entries. */
export function importSection(...entries: number[][]): number[] {
  return section(2, [...uleb(entries.length), ...entries.flat()]);
}

/** A module that does not import host.memory at all. */
export function moduleWithoutHostMemory(): Uint8Array<ArrayBuffer> {
  return new Uint8Array(WASM_HEADER);
}

// --- canned-module fixtures (shared by runtime and provider tests) -------------

export const CANNED_START = 0xe000;
export const CANNED_RESUME = 0xe400;
export const CANNED_DETAILS = 0xe800;

/** A byte-level module artifact: the canned guest plus a conforming metadata section. */
export function moduleArtifactBytes(
  clientSecret: string,
  variant = 1,
  fixture = 'canned-guest.wasm',
): Uint8Array<ArrayBuffer> {
  const digest = new Uint8Array(createHash('sha256').update(clientSecret, 'utf8').digest());
  const metadata = ResolverMetadata.encode({
    schemaVersion: 1,
    compilerVersion: '1.2.3',
    compilerGitSha: 'deadbeef',
    clientSecretSha256: [digest],
  }).finish();
  return new Uint8Array([
    ...loadFixture(fixture),
    ...customSection('confidence.metadata', metadata),
    ...(variant > 1 ? customSection('variant', [variant]) : []),
  ]);
}

/** Encrypt a ClientModule wrapper in the wire format: nonce || ciphertext || tag. */
export function encryptModuleWrapper(
  moduleBytes: Uint8Array,
  logToken: string,
  encryptionKey = '00'.repeat(32),
): Uint8Array<ArrayBuffer> {
  const wrapper = ClientModule.encode({ module: moduleBytes, logToken }).finish();
  const nonce = Buffer.alloc(12);
  const cipher = createCipheriv('aes-256-gcm', Buffer.from(encryptionKey, 'hex'), nonce);
  return new Uint8Array(Buffer.concat([nonce, cipher.update(wrapper), cipher.final(), cipher.getAuthTag()]));
}

export const cannedResolvedStart = ResolveProcessResponse.encode({
  resolved: {
    response: {
      resolvedFlags: [
        {
          flag: 'flags/test-flag',
          variant: 'control',
          value: { fields: { enabled: true } },
          reason: ResolveReason.RESOLVE_REASON_MATCH,
          shouldApply: true,
          assignmentOrigin: '',
        },
      ],
      resolveToken: new Uint8Array([1, 2, 3, 4]),
      resolveId: 'r1',
    },
    materializationToWrite: [],
    clientIndex: 7,
  },
}).finish();

export const cannedSuspended = ResolveProcessResponse.encode({
  suspended: {
    processId: 42,
    materializationToRead: [{ unit: 'user-1', materialization: 'm1', rule: 'r1', variant: '' }],
  },
}).finish();

export const cannedResolvedAfterResume = ResolveProcessResponse.encode({
  resolved: {
    response: {
      resolvedFlags: [
        {
          flag: 'flags/test-flag',
          variant: 'sticky',
          value: { fields: { enabled: false } },
          reason: ResolveReason.RESOLVE_REASON_MATCH,
          shouldApply: true,
          assignmentOrigin: '',
        },
      ],
      resolveToken: new Uint8Array([5, 6]),
      resolveId: 'r2',
    },
    materializationToWrite: [{ unit: 'user-1', materialization: 'm1', rule: 'r1', variant: 'sticky-1' }],
    clientIndex: 7,
  },
}).finish();

export const cannedError = ResolveProcessResponse.encode({
  error: { code: 16, message: 'invalid credential' },
}).finish();

export const cannedDetails = (overrides?: Partial<WasmResolutionDetails>): Uint8Array =>
  WasmResolutionDetails.encode({
    value: true,
    variant: 'on',
    reason: ResolveReason.RESOLVE_REASON_MATCH,
    errorCode: 0,
    errorMessage: '',
    shouldApply: true,
    ...overrides,
  }).finish();

/** Stage canned responses into the canned guest's fixed shared-memory slots. */
export function stageCannedResponses(
  memory: WebAssembly.Memory,
  start: Uint8Array,
  resume?: Uint8Array,
  details?: Uint8Array,
): void {
  const view = new Uint8Array(memory.buffer);
  const dataView = new DataView(memory.buffer);
  view.set(start, CANNED_START);
  dataView.setInt32(32, start.length, true);
  if (resume) {
    view.set(resume, CANNED_RESUME);
    dataView.setInt32(36, resume.length, true);
  }
  if (details) {
    view.set(details, CANNED_DETAILS);
    dataView.setInt32(40, details.length, true);
  }
}
