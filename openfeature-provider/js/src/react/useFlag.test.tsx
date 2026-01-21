/**
 * @vitest-environment jsdom
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import React from 'react';
import { useFlag, useFlagDetails } from './useFlag';
import { ConfidenceProvider } from './ConfidenceProvider';
import type { FlagBundle } from './types';

const createTestBundle = (flags: FlagBundle['flags'] = {}): FlagBundle => ({
  flags,
  resolveToken: 'test-token',
  resolveId: 'test-resolve-id',
});

describe('useFlag', () => {
  let mockApply: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockApply = vi.fn();
  });

  const wrapper =
    (bundle: FlagBundle) =>
    ({ children }: { children: React.ReactNode }) =>
      (
        <ConfidenceProvider bundle={bundle} apply={mockApply}>
          {children}
        </ConfidenceProvider>
      );

  describe('without provider', () => {
    it('returns default value when no provider is present', () => {
      const { result } = renderHook(() => useFlag('my-flag', 'default'));
      expect(result.current).toBe('default');
    });

    it('does not call apply when no provider is present', () => {
      renderHook(() => useFlag('my-flag', false));
      expect(mockApply).not.toHaveBeenCalled();
    });
  });

  describe('auto exposure (default)', () => {
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
    it('accesses nested property with single dot notation', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { enabled: true, limit: 100 },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag.enabled', false), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe(true);
    });

    it('accesses deeply nested property with multiple dots', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { config: { nested: { value: 'deep' } } },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag.config.nested.value', 'default'), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe('deep');
    });

    it('returns default value when nested path does not exist', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { enabled: true },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag.nonexistent.path', 'fallback'), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe('fallback');
    });

    it('returns default value when flag does not exist', () => {
      const bundle = createTestBundle({});

      const { result } = renderHook(() => useFlag('missing-flag.property', 'fallback'), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe('fallback');
    });

    it('calls apply with only the flag name, not the full path', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { enabled: true },
          reason: 'MATCH',
        },
      });

      renderHook(() => useFlag('my-flag.enabled', false), {
        wrapper: wrapper(bundle),
      });

      expect(mockApply).toHaveBeenCalledTimes(1);
      expect(mockApply).toHaveBeenCalledWith('my-flag');
    });

    it('handles numeric values in nested objects', () => {
      const bundle = createTestBundle({
        settings: {
          value: { limits: { max: 500, min: 10 } },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('settings.limits.max', 0), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe(500);
    });
  });

  describe('type validation', () => {
    it('returns default when flag value is string but default is boolean', () => {
      const bundle = createTestBundle({
        'my-flag': { value: 'not-a-boolean', reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlag('my-flag', false), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe(false);
    });

    it('returns default when flag value is number but default is string', () => {
      const bundle = createTestBundle({
        'my-flag': { value: 123, reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlag('my-flag', 'default'), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe('default');
    });

    it('returns default when flag value is boolean but default is number', () => {
      const bundle = createTestBundle({
        'my-flag': { value: true, reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlag('my-flag', 42), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBe(42);
    });

    it('returns default when object is missing required key', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { enabled: true },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag', { enabled: false, limit: 10 }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual({ enabled: false, limit: 10 });
    });

    it('returns default when object key has wrong type', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { enabled: 'yes', limit: 100 },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag', { enabled: false, limit: 0 }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual({ enabled: false, limit: 0 });
    });

    it('returns value when object has extra keys not in default', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { enabled: true, limit: 100, extra: 'ignored' },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag', { enabled: false, limit: 0 }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual({ enabled: true, limit: 100, extra: 'ignored' });
    });

    it('returns default when array is expected but object received', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { items: 'not-an-array' },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag', { items: [] as string[] }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual({ items: [] });
    });

    it('returns value when array items match expected type', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: ['a', 'b', 'c'],
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag', ['default']), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual(['a', 'b', 'c']);
    });

    it('returns default when array items have wrong type', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: [1, 2, 3],
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag', ['default']), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual(['default']);
    });

    it('returns value for empty array when default is empty array', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: [],
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag', [] as string[]), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual([]);
    });

    it('returns value for any array when default is empty array', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: [1, 2, 3],
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag', [] as number[]), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual([1, 2, 3]);
    });

    it('returns null when both value and default are null', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: null,
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag', null), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBeNull();
    });

    it('returns default when value is object but default is null', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { enabled: true },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag', null), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toBeNull();
    });

    it('returns default when value is null but default is object', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: null,
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag', { enabled: false }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual({ enabled: false });
    });

    it('validates nested object types recursively', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { outer: { inner: 'wrong-type' } },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag', { outer: { inner: 123 } }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual({ outer: { inner: 123 } });
    });

    it('validates deeply nested structures correctly', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { level1: { level2: { level3: { value: 'deep' } } } },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlag('my-flag', { level1: { level2: { level3: { value: '' } } } }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual({ level1: { level2: { level3: { value: 'deep' } } } });
    });
  });
});

describe('useFlagDetails', () => {
  let mockApply: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    mockApply = vi.fn();
  });

  const wrapper =
    (bundle: FlagBundle) =>
    ({ children }: { children: React.ReactNode }) =>
      (
        <ConfidenceProvider bundle={bundle} apply={mockApply}>
          {children}
        </ConfidenceProvider>
      );

  describe('auto exposure (default)', () => {
    it('returns value and undefined expose when auto-exposing', () => {
      const bundle = createTestBundle({
        'my-flag': { value: 'test-value', reason: 'MATCH' },
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
  });

  describe('manual exposure (expose: false)', () => {
    it('returns value and expose function', () => {
      const bundle = createTestBundle({
        'my-flag': { value: 'test-value', reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', 'default', { expose: false }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current).toEqual({
        value: 'test-value',
        expose: expect.any(Function),
      });
    });

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

    it('returns default value when flag is not in bundle', () => {
      const bundle = createTestBundle({});

      const { result } = renderHook(() => useFlagDetails('missing', 'fallback', { expose: false }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toBe('fallback');
    });
  });

  describe('dot notation', () => {
    it('accesses nested property with dot notation', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { enabled: true, config: { level: 5 } },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag.config.level', 0), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toBe(5);
    });

    it('calls apply with only the flag name for dot notation paths', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { nested: { prop: true } },
          reason: 'MATCH',
        },
      });

      renderHook(() => useFlagDetails('my-flag.nested.prop', false), {
        wrapper: wrapper(bundle),
      });

      expect(mockApply).toHaveBeenCalledTimes(1);
      expect(mockApply).toHaveBeenCalledWith('my-flag');
    });

    it('returns default value when nested path does not exist', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { enabled: true },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag.missing.path', 'default'), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toBe('default');
    });
  });

  describe('type validation', () => {
    it('returns default when flag value is string but default is boolean', () => {
      const bundle = createTestBundle({
        'my-flag': { value: 'not-a-boolean', reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', false), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toBe(false);
    });

    it('returns default when flag value is number but default is string', () => {
      const bundle = createTestBundle({
        'my-flag': { value: 123, reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', 'default'), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toBe('default');
    });

    it('returns default when object is missing required key', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { enabled: true },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', { enabled: false, limit: 10 }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toEqual({ enabled: false, limit: 10 });
    });

    it('returns default when object key has wrong type', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { enabled: 'yes', limit: 100 },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', { enabled: false, limit: 0 }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toEqual({ enabled: false, limit: 0 });
    });

    it('returns value when object has extra keys not in default', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { enabled: true, limit: 100, extra: 'included' },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', { enabled: false, limit: 0 }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toEqual({ enabled: true, limit: 100, extra: 'included' });
    });

    it('returns default when array items have wrong type', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: [1, 2, 3],
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', ['default']), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toEqual(['default']);
    });

    it('returns value when array items match expected type', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: ['a', 'b', 'c'],
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', ['default']), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toEqual(['a', 'b', 'c']);
    });

    it('returns null when both value and default are null', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: null,
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', null), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toBeNull();
    });

    it('returns default when value is null but default is object', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: null,
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', { enabled: false }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toEqual({ enabled: false });
    });

    it('validates nested object types recursively', () => {
      const bundle = createTestBundle({
        'my-flag': {
          value: { outer: { inner: 'wrong-type' } },
          reason: 'MATCH',
        },
      });

      const { result } = renderHook(() => useFlagDetails('my-flag', { outer: { inner: 123 } }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toEqual({ outer: { inner: 123 } });
    });
  });
});

describe('ConfidenceProvider', () => {
  it('provides context to children', () => {
    const bundle = createTestBundle({
      test: { value: 'provided', reason: 'MATCH' },
    });
    const mockApply = vi.fn();

    const { result } = renderHook(() => useFlag('test', 'default'), {
      wrapper: ({ children }) => (
        <ConfidenceProvider bundle={bundle} apply={mockApply}>
          {children}
        </ConfidenceProvider>
      ),
    });

    expect(result.current).toBe('provided');
  });
});
