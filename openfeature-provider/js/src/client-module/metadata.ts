import { ResolverMetadata } from '../proto/confidence/flags/wasm/v1/metadata';
import { ByteReader, verifyHeader } from './wasm-imports';
import { bytesEqual } from '../util';

/** Name of the custom section carrying routing metadata. */
export const RESOLVER_METADATA_SECTION = 'confidence.metadata';

/** The schema version this reader implements. */
export const RESOLVER_METADATA_SCHEMA_VERSION = 1;

/**
 * Read the `confidence.metadata` custom section from a compiled client
 * module's raw bytes — without decoding instructions, allocating instances,
 * or compiling. Mirrors the reference reader's contract:
 *
 * - returns `undefined` for a legacy module without metadata (tombstones are
 *   the caller's business);
 * - throws on invalid framing, duplicate metadata sections, malformed
 *   protobuf, unsupported schema versions, and shape violations.
 *
 * This is not a validator for the executable wasm sections.
 */
export function readResolverMetadata(wasm: Uint8Array): ResolverMetadata | undefined {
  verifyHeader(wasm);
  const reader = new ByteReader(wasm, 8);
  let found: ResolverMetadata | undefined;

  while (reader.position < wasm.length) {
    const sectionId = reader.byte('section id');
    const size = reader.uleb('section size');
    if (sectionId !== 0) {
      reader.skip(size, `section id ${sectionId}`);
      continue;
    }
    const previousEnd = reader.enter(size, 'custom section');
    const name = reader.name('custom section name');
    if (name !== RESOLVER_METADATA_SECTION) {
      reader.exit(previousEnd);
      continue;
    }
    if (found) {
      throw new Error('Duplicate resolver metadata');
    }
    const payload = reader.rest();
    reader.exit(previousEnd);
    found = decodeResolverMetadata(payload);
  }
  return found;
}

/** Shape validation, matching the reference reader: routing facts must be well-formed. */
export function validateResolverMetadata(metadata: ResolverMetadata): void {
  if (metadata.schemaVersion !== RESOLVER_METADATA_SCHEMA_VERSION) {
    throw new Error(`Unsupported resolver metadata schema: ${metadata.schemaVersion}`);
  }
  if (!metadata.compilerVersion) {
    throw new Error('Missing compiler version');
  }
  for (const digest of metadata.clientSecretSha256) {
    if (digest.length !== 32) {
      throw new Error('Invalid client-secret SHA-256');
    }
  }
  if (metadata.stateFileHash !== undefined && !/^[0-9a-f]{32}$/.test(metadata.stateFileHash)) {
    throw new Error('stateFileHash must be a 32-character lowercase hexadecimal MD5');
  }
}

/**
 * Reject a module that does not accept this credential: the artifact was
 * compiled for a different client secret, and the runtime would reject every
 * request anyway. `clientSecretSha256` is the SHA-256 of the exact UTF-8
 * bytes of the caller's client secret (32 raw bytes).
 */
export function assertAcceptsClientSecret(metadata: ResolverMetadata, clientSecretSha256: Uint8Array): void {
  const accepted = metadata.clientSecretSha256.some(digest => bytesEqual(digest, clientSecretSha256));
  if (!accepted) {
    throw new Error('Module does not accept this client secret');
  }
}

function decodeResolverMetadata(payload: Uint8Array): ResolverMetadata {
  let metadata: ResolverMetadata;
  try {
    metadata = ResolverMetadata.decode(payload);
  } catch (cause) {
    throw new Error(`Invalid resolver metadata protobuf: ${String(cause)}`);
  }
  validateResolverMetadata(metadata);
  return metadata;
}
