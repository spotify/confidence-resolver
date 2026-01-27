import { describe, it, expect } from 'vitest';
import * as FlagBundle from './flag-bundle';
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

      expect(parsed.resolveId).toBe('error');
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

      expect(bundle.resolveId).toBe('error');
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
