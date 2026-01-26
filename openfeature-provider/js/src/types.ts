export type ResolutionReason =
  | 'ERROR'
  | 'FLAG_ARCHIVED'
  | 'MATCH'
  | 'NO_SEGMENT_MATCH'
  | 'TARGETING_KEY_ERROR'
  | 'NO_TREATMENT_MATCH'
  | 'UNSPECIFIED';

export enum ErrorCode {
  PROVIDER_NOT_READY = 'PROVIDER_NOT_READY',
  PROVIDER_FATAL = 'PROVIDER_FATAL',
  FLAG_NOT_FOUND = 'FLAG_NOT_FOUND',
  TYPE_MISMATCH = 'TYPE_MISMATCH',
  GENERAL = 'GENERAL',
}
// These are OF errors we currently don't use
// PARSE_ERROR
// TARGETING_KEY_MISSING
// INVALID_CONTEXT

export interface ResolutionDetails<T> {
  reason: ResolutionReason;
  value: T;
  variant?: string;
  errorCode?: ErrorCode;
  errorMessage?: string;
  shouldApply: boolean;
}
