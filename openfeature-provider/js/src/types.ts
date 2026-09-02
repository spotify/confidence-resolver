export type ResolutionReason =
  | 'ERROR'
  | 'FLAG_ARCHIVED'
  | 'MATCH'
  | 'NO_SEGMENT_MATCH'
  | 'TARGETING_KEY_ERROR'
  | 'NO_TREATMENT_MATCH'
  | 'MATERIALIZATION_NOT_SUPPORTED'
  | 'UNSPECIFIED';

export enum ErrorCode {
  PROVIDER_NOT_READY = 'PROVIDER_NOT_READY',
  PROVIDER_FATAL = 'PROVIDER_FATAL',
  FLAG_NOT_FOUND = 'FLAG_NOT_FOUND',
  TYPE_MISMATCH = 'TYPE_MISMATCH',
  GENERAL = 'GENERAL',
}

export interface ResolutionDetails<T> {
  reason: ResolutionReason;
  value: T;
  variant?: string;
  errorCode?: ErrorCode;
  errorMessage?: string;
  shouldApply: boolean;
  assignmentOrigin?: string;
}

type FlagPrimitive = null | boolean | string | number;
export type FlagObject = {
  [key: string]: FlagValue;
};
export type FlagValue = FlagPrimitive | FlagObject;

/**
 * Structurally identical to `JsonValue` from `@openfeature/core`, restated here
 * so the standalone `./remote` entry point carries no OpenFeature dependency —
 * not even a type-only one, which TypeScript consumers would otherwise have to
 * install to resolve our `.d.ts`.
 */
export type JsonValue = FlagPrimitive | { [key: string]: JsonValue } | JsonValue[];
