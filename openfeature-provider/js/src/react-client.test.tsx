/**
 * @vitest-environment happy-dom
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import React from 'react';
import { useFlag, useFlagDetails, ConfidenceClientProvider } from './react-client';
import type { FlagBundle } from './types';

const createTestBundle = (flags: FlagBundle['flags'] = {}): FlagBundle => ({
  flags,
  resolveToken: 'test-token',
  resolveId: 'test-resolve-id',
});

describe('useFlag', () => {
  let mockApply: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockApply = vi.fn().mockResolvedValue(undefined);
  });

  const wrapper =
    (bundle: FlagBundle) =>
    ({ children }: { children: React.ReactNode }) =>
      (
        <ConfidenceClientProvider bundle={bundle} apply={mockApply}>
          {children}
        </ConfidenceClientProvider>
      );

  describe('without provider', () => {
    it('returns default value when no provider is present', () => {
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

      const { result } = renderHook(() => useFlag('no-provider-flag', 'default'));

      expect(result.current).toBe('default');
      expect(warnSpy).toHaveBeenCalledWith(
        expect.stringContaining('[Confidence] useFlagDetails("no-provider-flag") called without a ConfidenceProvider'),
      );

      warnSpy.mockRestore();
    });

    it('does not call apply when no provider is present', () => {
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

      renderHook(() => useFlag('no-provider-flag-2', false));

      expect(mockApply).not.toHaveBeenCalled();

      warnSpy.mockRestore();
    });

    it('only warns once per flag name', () => {
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

      const { rerender } = renderHook(() => useFlag('warn-once-flag', 'default'));
      rerender();
      rerender();

      expect(warnSpy).toHaveBeenCalledTimes(1);

      warnSpy.mockRestore();
    });
  });

  describe('auto exposure', () => {
    it('returns the flag value from the bundle', () => {
      const bundle = createTestBundle({
        'my-flag': { value: 'flag-value', variant: 'variant-a', reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlag('my-flag', 'default'), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe('flag-value');
    });

    it('returns default value when flag is not in bundle', () => {
      const bundle = createTestBundle({});

      const { result } = renderHook(() => useFlag('missing-flag', 'fallback'), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe('fallback');
    });

    it('returns flag value even when reason is not MATCH', () => {
      const bundle = createTestBundle({
        'my-flag': { value: 'flag-value', reason: 'STALE', errorCode: 'STALE' },
      });

      const { result } = renderHook(() => useFlag('my-flag', 'default'), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe('flag-value');
    });

    it('returns default value when flag value is undefined', () => {
      const bundle = createTestBundle({
        'my-flag': { value: undefined, reason: 'ERROR', errorCode: 'GENERAL' },
      });

      const { result } = renderHook(() => useFlag('my-flag', 'default'), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe('default');
    });

    it('calls apply on mount', () => {
      const bundle = createTestBundle({
        'my-flag': { value: true, reason: 'MATCH' },
      });

      renderHook(() => useFlag('my-flag', false), {
        wrapper: wrapper(bundle),
      });

      expect(mockApply).toHaveBeenCalledTimes(1);
      expect(mockApply).toHaveBeenCalledWith('my-flag');
    });

    it('only calls apply once even on re-render', () => {
      const bundle = createTestBundle({
        'my-flag': { value: true, reason: 'MATCH' },
      });

      const { rerender } = renderHook(() => useFlag('my-flag', false), {
        wrapper: wrapper(bundle),
      });

      rerender();
      rerender();

      expect(mockApply).toHaveBeenCalledTimes(1);
    });

    it('works with complex object values', () => {
      const bundle = createTestBundle({
        'config-flag': {
          value: { enabled: true, limit: 100, name: 'test' },
          variant: 'full-config',
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('config-flag', { enabled: false, limit: 0, name: '' }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual({ enabled: true, limit: 100, name: 'test' });
    });
  });

  describe('dot notation', () => {
    it('accesses nested property with dot notation', () => {
      const bundle = createTestBundle({
        'my-feature': {
          value: { config: { maxItems: 10, enabled: true } },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-feature.config.maxItems', 5), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe(10);
    });

    it('returns default when nested path does not exist', () => {
      const bundle = createTestBundle({
        'my-feature': {
          value: { config: { enabled: true } },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-feature.config.maxItems', 5), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe(5);
    });

    it('returns default when intermediate path is null', () => {
      const bundle = createTestBundle({
        'my-feature': {
          value: { config: null },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-feature.config.maxItems', 5), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe(5);
    });

    it('returns default when intermediate path is primitive', () => {
      const bundle = createTestBundle({
        'my-feature': {
          value: { config: 'not-an-object' },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-feature.config.maxItems', 5), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe(5);
    });

    it('calls apply with base flag name, not full path', () => {
      const bundle = createTestBundle({
        'my-feature': {
          value: { config: { maxItems: 10 } },
          reason: 'MATCH',
        },
      });

      renderHook(() => useFlag('my-feature.config.maxItems', 5), {
        wrapper: wrapper(bundle),
      });

      expect(mockApply).toHaveBeenCalledTimes(1);
      expect(mockApply).toHaveBeenCalledWith('my-feature');
    });
  });

  describe('type validation', () => {
    it('returns default when flag value type does not match', () => {
      const bundle = createTestBundle({
        'my-flag': { value: 'string-value', reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlag('my-flag', 42), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe(42);
    });

    it('returns default when object structure does not match', () => {
      const bundle = createTestBundle({
        'my-flag': { value: { enabled: true }, reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlag('my-flag', { enabled: false, limit: 0 }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual({ enabled: false, limit: 0 });
    });

    it('accepts value when object has extra properties', () => {
      const bundle = createTestBundle({
        'my-flag': { value: { enabled: true, limit: 100, extra: 'ignored' }, reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlag('my-flag', { enabled: false, limit: 0 }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual({ enabled: true, limit: 100, extra: 'ignored' });
    });

    it('validates array item types', () => {
      const bundle = createTestBundle({
        'my-flag': { value: [1, 2, 3], reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlag('my-flag', [0]), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual([1, 2, 3]);
    });

    it('returns default when array item types do not match', () => {
      const bundle = createTestBundle({
        'my-flag': { value: ['a', 'b', 'c'], reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlag('my-flag', [0]), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual([0]);
    });
  });
});

describe('useFlagDetails', () => {
  let mockApply: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockApply = vi.fn().mockResolvedValue(undefined);
  });

  const wrapper =
    (bundle: FlagBundle) =>
    ({ children }: { children: React.ReactNode }) =>
      (
        <ConfidenceClientProvider bundle={bundle} apply={mockApply}>
          {children}
        </ConfidenceClientProvider>
      );

  describe('auto exposure (default, expose: true)', () => {
    it('returns value and undefined expose', () => {
      const bundle = createTestBundle({
        'my-flag': { value: 'test-value', variant: 'variant-a', reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', 'default'), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toBe('test-value');
      expect(result.current.expose).toBeUndefined();
    });

    it('calls apply on mount', () => {
      const bundle = createTestBundle({
        'my-flag': { value: true, reason: 'MATCH' },
      });

      renderHook(() => useFlagDetails('my-flag', false), {
        wrapper: wrapper(bundle),
      });

      expect(mockApply).toHaveBeenCalledTimes(1);
      expect(mockApply).toHaveBeenCalledWith('my-flag');
    });

    it('returns default value when flag is not in bundle', () => {
      const bundle = createTestBundle({});

      const { result } = renderHook(() => useFlagDetails('missing-details', 'fallback'), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toBe('fallback');
    });
  });

  describe('manual exposure (expose: false)', () => {
    it('does not call apply on mount', () => {
      const bundle = createTestBundle({
        'my-flag': { value: true, reason: 'MATCH' },
      });

      renderHook(() => useFlagDetails('my-flag', false, { expose: false }), {
        wrapper: wrapper(bundle),
      });

      expect(mockApply).not.toHaveBeenCalled();
    });

    it('calls apply when expose is called', () => {
      const bundle = createTestBundle({
        'my-flag': { value: true, reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', false, { expose: false }), {
        wrapper: wrapper(bundle),
      });

      act(() => {
        result.current.expose!();
      });

      expect(mockApply).toHaveBeenCalledTimes(1);
      expect(mockApply).toHaveBeenCalledWith('my-flag');
    });

    it('only calls apply once even if expose is called multiple times', () => {
      const bundle = createTestBundle({
        'my-flag': { value: true, reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', false, { expose: false }), {
        wrapper: wrapper(bundle),
      });

      act(() => {
        result.current.expose!();
        result.current.expose!();
        result.current.expose!();
      });

      expect(mockApply).toHaveBeenCalledTimes(1);
    });

    it('returns value and expose function', () => {
      const bundle = createTestBundle({
        'my-flag': { value: 'test-value', reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', 'default', { expose: false }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toBe('test-value');
      expect(result.current.expose).toBeInstanceOf(Function);
    });
  });

  describe('with expose: true (explicit)', () => {
    it('behaves the same as default (auto exposure)', () => {
      const bundle = createTestBundle({
        'my-flag': { value: 'value', reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', 'default', { expose: true }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toBe('value');
      expect(result.current.expose).toBeUndefined();
      expect(mockApply).toHaveBeenCalledWith('my-flag');
    });
  });

  describe('dot notation with manual exposure', () => {
    it('applies with base flag name when using dot notation', () => {
      const bundle = createTestBundle({
        'my-feature': {
          value: { config: { maxItems: 10 } },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlagDetails('my-feature.config.maxItems', 5, { expose: false }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toBe(10);

      act(() => {
        result.current.expose!();
      });

      expect(mockApply).toHaveBeenCalledWith('my-feature');
    });
  });
});

describe('ConfidenceClientProvider', () => {
  it('provides context to children', () => {
    const bundle = createTestBundle({
      test: { value: 'provided', reason: 'MATCH' },
    });
    const mockApply = vi.fn().mockResolvedValue(undefined);

    const { result } = renderHook(() => useFlag('test', 'default'), {
      wrapper: ({ children }) => (
        <ConfidenceClientProvider bundle={bundle} apply={mockApply}>
          {children}
        </ConfidenceClientProvider>
      ),
    });

    expect(result.current).toBe('provided');
  });
});
