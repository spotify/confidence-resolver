/**
 * @vitest-environment jsdom
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import React from 'react';
import { useFlag } from './useFlag';
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

  describe('manual exposure (skipExposure: true)', () => {
    it('returns value and expose function', () => {
      const bundle = createTestBundle({
        'my-flag': { value: 'test-value', reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlag('my-flag', 'default', { skipExposure: true }), {
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

      renderHook(() => useFlag('my-flag', false, { skipExposure: true }), {
        wrapper: wrapper(bundle),
      });

      expect(mockApply).not.toHaveBeenCalled();
    });

    it('calls apply when expose is called', () => {
      const bundle = createTestBundle({
        'my-flag': { value: true, reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlag('my-flag', false, { skipExposure: true }), {
        wrapper: wrapper(bundle),
      });

      act(() => {
        result.current.expose();
      });

      expect(mockApply).toHaveBeenCalledTimes(1);
      expect(mockApply).toHaveBeenCalledWith('my-flag');
    });

    it('only calls apply once even if expose is called multiple times', () => {
      const bundle = createTestBundle({
        'my-flag': { value: true, reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlag('my-flag', false, { skipExposure: true }), {
        wrapper: wrapper(bundle),
      });

      act(() => {
        result.current.expose();
        result.current.expose();
        result.current.expose();
      });

      expect(mockApply).toHaveBeenCalledTimes(1);
    });

    it('returns default value when flag is not in bundle', () => {
      const bundle = createTestBundle({});

      const { result } = renderHook(() => useFlag('missing', 'fallback', { skipExposure: true }), {
        wrapper: wrapper(bundle),
      });

      expect(result.current.value).toBe('fallback');
    });
  });

  describe('with skipExposure: false (explicit)', () => {
    it('behaves the same as default (auto exposure)', () => {
      const bundle = createTestBundle({
        'my-flag': { value: 'value', reason: 'MATCH' },
      });

      const { result } = renderHook(() => useFlag('my-flag', 'default', { skipExposure: false }), {
        wrapper: wrapper(bundle),
      });

      // Returns value directly, not { value, expose }
      expect(result.current).toBe('value');
      expect(mockApply).toHaveBeenCalledWith('my-flag');
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
