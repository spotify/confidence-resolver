import { describe, it, expect } from 'vitest';
import * as FlagBundle from './flag-bundle';
import { evaluateAssignment } from './flag-bundle';
import { ResolveReason } from './proto/confidence/flags/resolver/v1/types';
import { ErrorCode } from './types';

describe('FlagBundle', () => {
  /**
   * React Server Components serialize props as JSON when passing data from
   * server to client components. Uint8Array cannot be serialized to JSON/UTF-8,
   * causing "TypeError: The encoded data was not valid for encoding utf-8".
   *
   * These tests verify that FlagBundle uses base64-encoded strings for binary
   * data (resolveToken), ensuring RSC compatibility.
   */
  describe('JSON serialization (RSC compatibility)', () => {
    it('bundle from create() is JSON serializable', () => {
      const bundle = FlagBundle.create({
        resolveId: 'test-resolve-id',
        resolveToken: new Uint8Array([1, 2, 3, 255, 254, 253]),
        resolvedFlags: [
          {
            flag: 'flags/test-flag',
            variant: 'control',
            value: { enabled: true },
            reason: ResolveReason.RESOLVE_REASON_MATCH,
            shouldApply: true,
          },
        ],
      });

      // This would throw if Uint8Array was in the bundle
      const json = JSON.stringify(bundle);
      const parsed = JSON.parse(json);

      expect(parsed.resolveId).toBe('test-resolve-id');
      expect(typeof parsed.resolveToken).toBe('string');
      expect(parsed.flags['test-flag'].value).toEqual({ enabled: true });
    });

    it('bundle from error() is JSON serializable', () => {
      const bundle = FlagBundle.error(ErrorCode.GENERAL, 'Something went wrong');

      const json = JSON.stringify(bundle);
      const parsed = JSON.parse(json);

      expect(parsed.resolveId).toBe('');
      expect(parsed.resolveToken).toBe('');
      expect(parsed.errorCode).toBe(ErrorCode.GENERAL);
      expect(parsed.errorMessage).toBe('Something went wrong');
    });

    it('resolveToken survives JSON round-trip and can be decoded', () => {
      const originalBytes = new Uint8Array([0, 1, 2, 128, 255]);
      const bundle = FlagBundle.create({
        resolveId: 'test',
        resolveToken: originalBytes,
        resolvedFlags: [],
      });

      // Simulate RSC serialization: server component passes bundle to client component
      const json = JSON.stringify(bundle);
      const parsed = JSON.parse(json);

      // Simulate server action: client calls applyFlag, server decodes token
      const decodedBytes = FlagBundle.decodeToken(parsed.resolveToken);
      expect(decodedBytes).toEqual(originalBytes);
    });

    it('handles binary data with all byte values', () => {
      const allBytes = new Uint8Array(256);
      for (let i = 0; i < 256; i++) {
        allBytes[i] = i;
      }

      const bundle = FlagBundle.create({
        resolveId: 'test',
        resolveToken: allBytes,
        resolvedFlags: [],
      });

      const json = JSON.stringify(bundle);
      const parsed = JSON.parse(json);
      const decoded = FlagBundle.decodeToken(parsed.resolveToken);

      expect(decoded).toEqual(allBytes);
    });
  });

  describe('create()', () => {
    it('extracts flag name from flag path', () => {
      const bundle = FlagBundle.create({
        resolveId: 'test',
        resolveToken: new Uint8Array(),
        resolvedFlags: [
          {
            flag: 'flags/my-feature',
            variant: 'v1',
            value: { key: 'value' },
            reason: ResolveReason.RESOLVE_REASON_MATCH,
            shouldApply: true,
          },
        ],
      });

      expect(bundle.flags['my-feature']).toBeDefined();
      expect(bundle.flags['flags/my-feature']).toBeUndefined();
    });

    it('converts resolve reason correctly', () => {
      const bundle = FlagBundle.create({
        resolveId: 'test',
        resolveToken: new Uint8Array(),
        resolvedFlags: [
          {
            flag: 'flags/test',
            variant: 'v1',
            value: undefined,
            reason: ResolveReason.RESOLVE_REASON_NO_SEGMENT_MATCH,
            shouldApply: false,
          },
        ],
      });

      expect(bundle.flags['test']?.reason).toBe('NO_SEGMENT_MATCH');
    });

    it('preserves shouldApply from response', () => {
      const bundle = FlagBundle.create({
        resolveId: 'test',
        resolveToken: new Uint8Array(),
        resolvedFlags: [
          {
            flag: 'flags/apply-true',
            variant: 'v1',
            value: { x: 1 },
            reason: ResolveReason.RESOLVE_REASON_MATCH,
            shouldApply: true,
          },
          {
            flag: 'flags/apply-false',
            variant: 'v1',
            value: { x: 2 },
            reason: ResolveReason.RESOLVE_REASON_MATCH,
            shouldApply: false,
          },
        ],
      });

      expect(bundle.flags['apply-true']?.shouldApply).toBe(true);
      expect(bundle.flags['apply-false']?.shouldApply).toBe(false);
    });
  });

  describe('error()', () => {
    it('creates bundle with error state', () => {
      const bundle = FlagBundle.error(ErrorCode.FLAG_NOT_FOUND, 'Flag does not exist');

      expect(bundle.resolveId).toBe('');
      expect(bundle.resolveToken).toBe('');
      expect(bundle.errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
      expect(bundle.errorMessage).toBe('Flag does not exist');
      expect(bundle.flags).toEqual({});
    });
  });

  describe('resolve()', () => {
    it('returns error when bundle has errorCode', () => {
      const bundle = FlagBundle.error(ErrorCode.GENERAL, 'Test error');
      const result = FlagBundle.resolve(bundle, 'any-flag', 'default');

      expect(result.value).toBe('default');
      expect(result.reason).toBe('ERROR');
      expect(result.errorCode).toBe(ErrorCode.GENERAL);
      expect(result.shouldApply).toBe(false);
    });

    it('returns FLAG_NOT_FOUND when flag is missing', () => {
      const bundle = FlagBundle.create({
        resolveId: 'test',
        resolveToken: new Uint8Array(),
        resolvedFlags: [],
      });

      const result = FlagBundle.resolve(bundle, 'missing-flag', 'default');

      expect(result.value).toBe('default');
      expect(result.errorCode).toBe(ErrorCode.FLAG_NOT_FOUND);
      expect(result.shouldApply).toBe(false);
    });

    it('resolves nested property with dot notation', () => {
      const bundle = FlagBundle.create({
        resolveId: 'test',
        resolveToken: new Uint8Array(),
        resolvedFlags: [
          {
            flag: 'flags/my-flag',
            variant: 'v1',
            value: { config: { enabled: true, limit: 10 } },
            reason: ResolveReason.RESOLVE_REASON_MATCH,
            shouldApply: true,
          },
        ],
      });

      const enabled = FlagBundle.resolve(bundle, 'my-flag.config.enabled', false);
      const limit = FlagBundle.resolve(bundle, 'my-flag.config.limit', 0);

      expect(enabled.value).toBe(true);
      expect(limit.value).toBe(10);
    });
  });
});

describe('evaluateAssignment', () => {
  describe('primitives', () => {
    it('returns resolved value when types match', () => {
      expect(evaluateAssignment(42, 0, ['flag'])).toBe(42);
      expect(evaluateAssignment('hello', '', ['flag'])).toBe('hello');
      expect(evaluateAssignment(true, false, ['flag'])).toBe(true);
    });

    it('throws when types do not match', () => {
      expect(() => evaluateAssignment('string', 0, ['flag'])).toThrow(
        "resolved value (string) isn't assignable to default type (number) at flag",
      );
      expect(() => evaluateAssignment(42, '', ['flag'])).toThrow(
        "resolved value (number) isn't assignable to default type (string) at flag",
      );
    });

    it('returns default when resolved is null', () => {
      expect(evaluateAssignment(null, 42, ['flag'])).toBe(42);
      expect(evaluateAssignment(null, 'default', ['flag'])).toBe('default');
      expect(evaluateAssignment(null, false, ['flag'])).toBe(false);
    });
  });

  describe('null default (accept any)', () => {
    it('returns resolved value regardless of type', () => {
      expect(evaluateAssignment(42, null, ['flag'])).toBe(42);
      expect(evaluateAssignment('hello', null, ['flag'])).toBe('hello');
      expect(evaluateAssignment({ a: 1 }, null, ['flag'])).toEqual({ a: 1 });
      expect(evaluateAssignment(null, null, ['flag'])).toBe(null);
    });
  });

  describe('objects', () => {
    it('returns resolved object when structure matches', () => {
      const resolved = { enabled: true, count: 5 };
      const defaultValue = { enabled: false, count: 0 };
      expect(evaluateAssignment(resolved, defaultValue, ['flag'])).toEqual(resolved);
    });

    it('preserves extra fields from resolved object', () => {
      const resolved = { enabled: true, count: 5, extra: 'bonus' };
      const defaultValue = { enabled: false, count: 0 };
      expect(evaluateAssignment(resolved, defaultValue, ['flag'])).toEqual(resolved);
    });

    it('throws when required field is missing', () => {
      const resolved = { enabled: true };
      const defaultValue = { enabled: false, count: 0 };
      expect(() => evaluateAssignment(resolved, defaultValue, ['flag'])).toThrow(
        'resolved value is missing field "count" at flag',
      );
    });

    it('throws when field type mismatches', () => {
      const resolved = { enabled: 'yes', count: 5 };
      const defaultValue = { enabled: false, count: 0 };
      expect(() => evaluateAssignment(resolved, defaultValue, ['flag'])).toThrow(
        "resolved value (string) isn't assignable to default type (boolean) at flag.enabled",
      );
    });

    it('substitutes default for null fields', () => {
      const resolved = { enabled: null, count: 5 };
      const defaultValue = { enabled: true, count: 0 };
      expect(evaluateAssignment(resolved, defaultValue, ['flag'])).toEqual({
        enabled: true,
        count: 5,
      });
    });

    it('recursively substitutes defaults for nested null fields', () => {
      const resolved = { config: { enabled: null, label: 'test' } };
      const defaultValue = { config: { enabled: true, label: '' } };
      expect(evaluateAssignment(resolved, defaultValue, ['flag'])).toEqual({
        config: { enabled: true, label: 'test' },
      });
    });
  });

  describe('arrays', () => {
    it('throws when default value is an array', () => {
      expect(() => evaluateAssignment({ a: 1 }, [0], ['flag'])).toThrow(
        'arrays are not supported as flag values at flag',
      );
    });

    it('throws when default value contains a nested array', () => {
      expect(() => evaluateAssignment({ items: { a: 1 } }, { items: [0] }, ['flag'])).toThrow(
        'arrays are not supported as flag values at flag.items',
      );
    });
  });

  describe('deeply nested structures', () => {
    it('handles deeply nested objects with null substitution', () => {
      const resolved = {
        level1: {
          level2: {
            level3: {
              value: null,
              other: 'kept',
            },
          },
        },
      };
      const defaultValue = {
        level1: {
          level2: {
            level3: {
              value: 42,
              other: '',
            },
          },
        },
      };
      expect(evaluateAssignment(resolved, defaultValue, ['flag'])).toEqual({
        level1: {
          level2: {
            level3: {
              value: 42,
              other: 'kept',
            },
          },
        },
      });
    });

    it('includes path in error messages for nested failures', () => {
      const resolved = { a: { b: { c: 'wrong' } } };
      const defaultValue = { a: { b: { c: 123 } } };
      expect(() => evaluateAssignment(resolved, defaultValue, ['flag'])).toThrow(
        "resolved value (string) isn't assignable to default type (number) at flag.a.b.c",
      );
    });
  });
});
