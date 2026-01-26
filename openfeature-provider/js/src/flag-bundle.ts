import type { JsonObject, JsonValue } from '@openfeature/core';
import type { ResolveFlagsResponse } from './proto/confidence/flags/resolver/v1/api';
import { ResolveReason } from './proto/confidence/flags/resolver/v1/types';
import { hasKey } from './util';
import { ErrorCode, ResolutionDetails, ResolutionReason } from './types';
import { Logger } from './logger';

const FLAG_PREFIX = 'flags/';

export default interface FlagBundle {
  flags: Record<string, ResolutionDetails<JsonObject | null> | undefined>;
  resolveId: string;
  resolveToken: Uint8Array;
  error?: unknown;
}

export function create({ resolveId, resolveToken, resolvedFlags }: ResolveFlagsResponse): FlagBundle {
  const flags = Object.fromEntries(
    resolvedFlags.map(({ flag, reason, variant, value, shouldApply }) => {
      const name = flag.slice(FLAG_PREFIX.length);
      const details: ResolutionDetails<JsonObject | null> = {
        reason: convertReason(reason),
        variant,
        value: value ?? null,
        shouldApply,
      };
      return [name, details];
    }),
  );

  return {
    flags,
    resolveId,
    resolveToken,
  };
}

export function error(e: unknown): FlagBundle {
  return {
    flags: {},
    resolveId: 'error',
    resolveToken: new Uint8Array(),
    error: e,
  };
}

export function resolve<T extends JsonValue>(
  bundle: FlagBundle | undefined,
  flagKey: string,
  defaultValue: T,
  logger?: Logger,
): ResolutionDetails<T> {
  const [flagName, ...path] = flagKey.split('.');
  const flag = bundle?.flags[flagName];
  const error = bundle?.error;
  if (error) {
    logger?.warn(`Flag evaluation for '${flagKey}' failed`, error);
    return {
      reason: 'ERROR',
      errorCode: ErrorCode.GENERAL,
      errorMessage: String(error),
      value: defaultValue,
      shouldApply: false,
    };
  }

  if (!flag) {
    logger?.warn(`Flag evaluation for '${flagKey}' failed: flag not found`);
    return {
      reason: 'ERROR',
      errorCode: ErrorCode.FLAG_NOT_FOUND,
      value: defaultValue,
      shouldApply: false,
    };
  }

  let value: JsonValue = flag.value;
  for (let i = 0; i < path.length; i++) {
    if (value === null || typeof value !== 'object' || Array.isArray(value)) {
      return {
        reason: 'ERROR',
        value: defaultValue,
        errorCode: ErrorCode.TYPE_MISMATCH,
        errorMessage: `resolved value is not an object at ${[flagName, ...path.slice(0, i)].join('.')}`,
        shouldApply: false,
      };
    }
    value = value[path[i]];
  }

  try {
    validateAssignment(value, defaultValue, [flagName, ...path]);
  } catch (e) {
    return {
      reason: 'ERROR',
      value: defaultValue,
      errorCode: ErrorCode.TYPE_MISMATCH,
      errorMessage: String(e),
      shouldApply: false,
    };
  }

  return {
    ...flag,
    value,
  };
}

export function validateAssignment<T extends JsonValue>(
  resolvedValue: JsonValue,
  defaultValue: T,
  path: string[],
): asserts resolvedValue is T {
  const resolvedType = typeof resolvedValue;
  const defaultType = typeof defaultValue;

  if (defaultValue === null) return;
  if (resolvedValue === null) {
    // is this correct? null isn't assignable to anything?
    // I think it is. It'd be very annoying if your defaultValue is a number, but instead you resolve null!
    // The alt. would be actually merge in this case. I.e. we swap the null for the default value.
    throw `resolved value (null) isn't assignable to default type (${defaultType}) at ${path.join('.')}`;
  }
  if (resolvedType !== defaultType) {
    throw `resolved value (${resolvedType}) isn't assignable to default type (${defaultType}) at ${path.join('.')}`;
  }
  if (typeof resolvedValue === 'object') {
    // we know defaultValue is also 'object'
    if (Array.isArray(defaultValue)) {
      if (!Array.isArray(resolvedValue)) {
        throw `resolved value (${resolvedType}) isn't assignable to default type (array) at ${path.join('.')}`;
      }
      const defaultItem = defaultValue[0] ?? null;
      resolvedValue.forEach(resolvedItem => validateAssignment(resolvedItem, defaultItem, path));
      return;
    }

    for (const [key, value] of Object.entries(defaultValue)) {
      if (!hasKey(resolvedValue, key)) {
        throw `resolved value is missing field "${key}" at ${path.join('.')}`;
      }
      validateAssignment(resolvedValue[key], value, [...path, key]);
    }
  }
}

function convertReason(reason: ResolveReason): ResolutionReason {
  switch (reason) {
    case ResolveReason.RESOLVE_REASON_ERROR:
      return 'ERROR';
    case ResolveReason.RESOLVE_REASON_FLAG_ARCHIVED:
      return 'FLAG_ARCHIVED';
    case ResolveReason.RESOLVE_REASON_MATCH:
      return 'MATCH';
    case ResolveReason.RESOLVE_REASON_NO_SEGMENT_MATCH:
      return 'NO_SEGMENT_MATCH';
    case ResolveReason.RESOLVE_REASON_TARGETING_KEY_ERROR:
      return 'TARGETING_KEY_ERROR';
    case ResolveReason.RESOLVE_REASON_NO_TREATMENT_MATCH:
      return 'NO_TREATMENT_MATCH';
    default:
      return 'UNSPECIFIED';
  }
}
