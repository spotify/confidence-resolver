import { describe, expect, it } from 'vitest';
import { ResolverMetadata } from './proto/confidence/flags/wasm/v1/metadata';
import {
  assertAcceptsClientSecret,
  readResolverMetadata,
  RESOLVER_METADATA_SECTION,
  validateResolverMetadata,
} from './metadata';
import { customSection, wasm } from './test-helpers';
import { readSharedMemoryLimits } from './wasm-imports';
import { hostMemoryImport, importSection } from './test-helpers';

function digestOf(fill: number): Uint8Array {
  return new Uint8Array(32).fill(fill);
}

function metadataMessage(overrides?: Partial<ResolverMetadata>): ResolverMetadata {
  return {
    schemaVersion: 1,
    compilerVersion: '1.2.3',
    compilerGitSha: '0123456789abcdef',
    clientSecretSha256: [digestOf(1), digestOf(2)],
    ...overrides,
  };
}

function moduleWithMetadata(metadata: ResolverMetadata, ...sections: number[][]): Uint8Array {
  const payload = ResolverMetadata.encode(metadataMessage(metadata)).finish();
  return wasm(...sections, customSection(RESOLVER_METADATA_SECTION, payload));
}

const DEFAULT_METADATA = metadataMessage();

describe('readResolverMetadata', () => {
  it('reads metadata from a custom section after other sections', () => {
    const module = moduleWithMetadata(DEFAULT_METADATA, importSection(hostMemoryImport(1, 2)));
    const metadata = readResolverMetadata(module);
    expect(metadata).toEqual(DEFAULT_METADATA);
  });

  it('reads metadata from a custom section before other sections', () => {
    const payload = ResolverMetadata.encode(DEFAULT_METADATA).finish();
    const module = wasm(customSection(RESOLVER_METADATA_SECTION, payload), importSection(hostMemoryImport(1, 2)));
    expect(readResolverMetadata(module)).toEqual(DEFAULT_METADATA);
  });

  it('returns undefined for a module without the section (legacy modules)', () => {
    expect(readResolverMetadata(wasm())).toBeUndefined();
    expect(readResolverMetadata(wasm(importSection(hostMemoryImport(1, 2))))).toBeUndefined();
  });

  it('ignores other custom sections (e.g. the name section)', () => {
    const module = wasm(customSection('name', [0x01, 0x02]), customSection('sourceMappingURL', [0x03]));
    expect(readResolverMetadata(module)).toBeUndefined();
  });

  it('rejects duplicate metadata sections', () => {
    const payload = ResolverMetadata.encode(DEFAULT_METADATA).finish();
    const module = wasm(
      customSection(RESOLVER_METADATA_SECTION, payload),
      customSection(RESOLVER_METADATA_SECTION, payload),
    );
    expect(() => readResolverMetadata(module)).toThrow(/Duplicate resolver metadata/);
  });

  it('rejects a malformed protobuf payload', () => {
    const module = wasm(customSection(RESOLVER_METADATA_SECTION, [0x0a, 0xff, 0xff, 0xff]));
    expect(() => readResolverMetadata(module)).toThrow(/Invalid resolver metadata protobuf/);
  });

  it('rejects unsupported schema versions', () => {
    const module = moduleWithMetadata(metadataMessage({ schemaVersion: 2 }));
    expect(() => readResolverMetadata(module)).toThrow(/Unsupported resolver metadata schema: 2/);
  });

  it('rejects a blank compiler version', () => {
    const module = moduleWithMetadata(metadataMessage({ compilerVersion: '' }));
    expect(() => readResolverMetadata(module)).toThrow(/Missing compiler version/);
  });

  it('rejects client-secret digests that are not 32 bytes', () => {
    const module = moduleWithMetadata(metadataMessage({ clientSecretSha256: [new Uint8Array(31)] }));
    expect(() => readResolverMetadata(module)).toThrow(/Invalid client-secret SHA-256/);
  });

  it('rejects malformed state file hashes', () => {
    for (const stateFileHash of ['XYZ', '0123456789ABCDEF0123456789ABCDEF', '0123456789abcdef0123456789abcde']) {
      const module = moduleWithMetadata(metadataMessage({ stateFileHash }));
      expect(() => readResolverMetadata(module), `hash=${stateFileHash}`).toThrow(/32-character lowercase hexadecimal/);
    }
  });

  it('accepts a valid state file hash and absent state file hash', () => {
    expect(
      readResolverMetadata(moduleWithMetadata(metadataMessage({ stateFileHash: '0123456789abcdef0123456789abcdef' }))),
    ).toEqual({ ...DEFAULT_METADATA, stateFileHash: '0123456789abcdef0123456789abcdef' });
  });

  it('rejects bad wasm framing', () => {
    expect(() => readResolverMetadata(new Uint8Array([0x00, 0x61]))).toThrow(/shorter than the 8-byte header/);
    // custom section whose size overruns the binary
    const module = new Uint8Array([...wasm(), 0x00, 0x7f]);
    expect(() => readResolverMetadata(module)).toThrow(/extends past the end/);
    // section name longer than the section
    const truncated = new Uint8Array([
      ...wasm(),
      0x00, // custom section
      0x03, // size: 3
      0x05, // name length: 5 — only 2 name bytes follow
      0x63,
      0x6f, // "co"
    ]);
    expect(() => readResolverMetadata(truncated)).toThrow(/Truncated/);
  });

  it('coexists with the import-section parser', () => {
    const module = moduleWithMetadata(DEFAULT_METADATA, importSection(hostMemoryImport(2, 4)));
    expect(readSharedMemoryLimits(module)).toEqual({ initial: 2, maximum: 4 });
    expect(readResolverMetadata(module)).toEqual(DEFAULT_METADATA);
  });
});

describe('assertAcceptsClientSecret', () => {
  it('accepts a credential whose digest is listed', () => {
    expect(() => assertAcceptsClientSecret(DEFAULT_METADATA, digestOf(2))).not.toThrow();
  });

  it('rejects a credential whose digest is not listed', () => {
    expect(() => assertAcceptsClientSecret(DEFAULT_METADATA, digestOf(9))).toThrow(
      /does not accept this client secret/,
    );
    const empty = metadataMessage({ clientSecretSha256: [] });
    expect(() => assertAcceptsClientSecret(empty, digestOf(1))).toThrow(/does not accept this client secret/);
  });
});

describe('validateResolverMetadata', () => {
  it('passes for a well-formed message', () => {
    expect(() => validateResolverMetadata(DEFAULT_METADATA)).not.toThrow();
  });
});
