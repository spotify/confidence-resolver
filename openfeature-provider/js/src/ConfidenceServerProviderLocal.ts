/**
 * Public provider API. The implementation is the compiled client-module
 * runtime (`./client-module/provider`); this module keeps the historical
 * import path stable for the package and its consumers.
 */
export {
  ConfidenceServerProviderLocal,
  DEFAULT_FLUSH_INTERVAL,
  DEFAULT_INITIALIZE_TIMEOUT,
} from './client-module/provider';
export type { ProviderOptions, SnapshotConfig } from './client-module/provider';
export { validateEncryptionKey } from './util';
