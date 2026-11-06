import { ClientModule } from './proto/confidence/flags/wasm/v1/client_module';
import { ResolverMetadata } from './proto/confidence/flags/wasm/v1/metadata';
import { assertAcceptsClientSecret, readResolverMetadata } from './metadata';
import { sha256Bytes } from './hash';
import { bytesToHex, bytesEqual, hexToBytes, decryptAesGcm, validateEncryptionKey } from './util';

/** Default module refresh poll interval (the guide recommends ~30s). */
export const DEFAULT_MODULE_UPDATE_INTERVAL = 30_000;

/** Default module CDN; sibling of the per-client state CDN. */
export const DEFAULT_MODULE_BASE_URL = 'https://confidence-resolver-module-cdn.spotifycdn.com';

export interface ModuleClientOptions {
  /** The credential's client secret; also derives the module path. */
  clientSecret: string;
  /** Hex-encoded AES-256 encryption key paired with the credential. */
  encryptionKey: string;
  /** Module CDN base URL. Default: the production CDN. */
  baseUrl?: string;
  /** Custom fetch (tests, custom transports). */
  fetch?: typeof fetch;
}

/** The currently accepted artifact snapshot. */
export interface CurrentModule {
  /** Raw wasm module bytes (the wrapper's `module` field; an owned copy). */
  moduleBytes: Uint8Array<ArrayBuffer>;
  /** Routing metadata verified for this artifact. */
  metadata: ResolverMetadata;
  /** Bare JWT from the wrapper; the SDK adds the Authorization header when forwarding logs. Never log it. */
  logToken: string;
  /** ETag of the accepted fetch; sent as If-None-Match on the next refresh. */
  etag: string | null;
}

export type ModuleRefreshOutcome =
  /** 304: the artifact is unchanged. */
  | { type: 'unchanged' }
  /** 404: no module published for this credential (yet, or any more). */
  | { type: 'not-published' }
  /**
   * A new artifact was accepted. `change` is `initial` for the first module,
   * `module` for a new module body (rebuild pools), and `token` for a
   * wrapper-only change (same module bytes identity, new log token: swap the
   * token for new batches, recompile nothing).
   */
  | { type: 'accepted'; change: 'initial' | 'module' | 'token'; current: CurrentModule }
  /**
   * The cycle failed (fetch, decrypt, invalid wrapper, bad metadata, wrong
   * credential). The last accepted module keeps serving; the provider is not
   * taken down.
   */
  | { type: 'failed'; reason: string };

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Fetches, decrypts and verifies one credential's compiled client module, and
 * tracks the currently accepted artifact.
 *
 * The module path is derived from the client secret (`<base>/modules/<sha256
 * of the secret's exact UTF-8 bytes>.wasm.enc`); the wrapper is decrypted with
 * the credential's AES-256-GCM key; the module's `confidence.metadata` section
 * must accept this credential. A failed cycle never discards the last
 * accepted module — only a successfully verified replacement does.
 */
export class ModuleClient {
  private readonly baseUrl: string;
  private readonly fetchImpl: typeof fetch;
  private readonly rawKey: Uint8Array;
  private readonly secretDigest: Promise<Uint8Array>;
  private currentField: CurrentModule | null = null;
  private refreshChain: Promise<void> = Promise.resolve();

  constructor(private readonly options: ModuleClientOptions) {
    validateEncryptionKey(options.encryptionKey);
    if (!options.clientSecret) {
      throw new Error('clientSecret is required');
    }
    this.baseUrl = (options.baseUrl ?? DEFAULT_MODULE_BASE_URL).replace(/\/+$/, '');
    this.fetchImpl = options.fetch ?? fetch;
    this.rawKey = hexToBytes(options.encryptionKey);
    this.secretDigest = sha256Bytes(options.clientSecret);
  }

  /** The currently accepted artifact, or null before the first accept. */
  get current(): CurrentModule | null {
    return this.currentField;
  }

  /**
   * Run one refresh cycle. Never throws except on an aborted `signal`: every
   * other failure is reported as a `failed` outcome while the last accepted
   * module keeps serving. Concurrent calls are serialized.
   */
  refresh(signal?: AbortSignal): Promise<ModuleRefreshOutcome> {
    const cycle = this.refreshChain.then(() => this.doRefresh(signal));
    this.refreshChain = cycle.then(
      () => undefined,
      () => undefined,
    );
    return cycle;
  }

  private async doRefresh(signal?: AbortSignal): Promise<ModuleRefreshOutcome> {
    const url = `${this.baseUrl}/modules/${bytesToHex(await this.secretDigest)}.wasm.enc`;
    const headers = new Headers();
    if (this.currentField?.etag) {
      headers.set('If-None-Match', this.currentField.etag);
    }

    let response: Response;
    try {
      response = await this.fetchImpl(url, { headers, signal });
    } catch (error) {
      if (signal?.aborted) throw error;
      return { type: 'failed', reason: `fetch failed: ${errorMessage(error)}` };
    }

    if (response.status === 304) {
      return { type: 'unchanged' };
    }
    if (response.status === 404) {
      return { type: 'not-published' };
    }
    if (!response.ok) {
      return { type: 'failed', reason: `unexpected status ${response.status} ${response.statusText}` };
    }

    let encrypted: Uint8Array;
    try {
      encrypted = new Uint8Array(await response.arrayBuffer());
    } catch (error) {
      if (signal?.aborted) throw error;
      return { type: 'failed', reason: `failed to read response body: ${errorMessage(error)}` };
    }

    let wrapper: ClientModule;
    try {
      signal?.throwIfAborted();
      wrapper = ClientModule.decode(await decryptAesGcm(encrypted, this.rawKey));
    } catch (error) {
      if (signal?.aborted) throw error;
      return { type: 'failed', reason: `failed to decrypt module wrapper: ${errorMessage(error)}` };
    }
    if (wrapper.module.length === 0) {
      return { type: 'failed', reason: 'module wrapper has an empty module body' };
    }

    let metadata: ResolverMetadata;
    try {
      const parsed = readResolverMetadata(wrapper.module);
      if (!parsed) {
        return { type: 'failed', reason: 'module has no confidence.metadata section (legacy or tombstone)' };
      }
      assertAcceptsClientSecret(parsed, await this.secretDigest);
      metadata = parsed;
    } catch (error) {
      return { type: 'failed', reason: errorMessage(error) };
    }

    const etag = response.headers.get('etag');
    const previous = this.currentField;
    if (previous && bytesEqual(previous.moduleBytes, wrapper.module)) {
      // Wrapper-only change: keep the accepted module bytes (identity-stable
      // for consumers), swap the token for newly queued batches.
      this.currentField = {
        moduleBytes: previous.moduleBytes,
        metadata: previous.metadata,
        logToken: wrapper.logToken,
        etag,
      };
      return { type: 'accepted', change: 'token', current: this.currentField };
    }
    this.currentField = {
      moduleBytes: new Uint8Array(wrapper.module),
      metadata,
      logToken: wrapper.logToken,
      etag,
    };
    return { type: 'accepted', change: previous ? 'module' : 'initial', current: this.currentField };
  }
}
