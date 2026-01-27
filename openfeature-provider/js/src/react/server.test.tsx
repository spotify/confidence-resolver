/**
 * @vitest-environment happy-dom
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { OpenFeature } from '@openfeature/server-sdk';
import type { Provider, EvaluationContext } from '@openfeature/server-sdk';
import React from 'react';
import { ConfidenceProvider } from './server';
import type { ConfidenceServerProviderLocal } from '../ConfidenceServerProviderLocal';

// Mock provider that matches ConfidenceServerProviderLocal's metadata
function createMockConfidenceProvider(
  overrides: Partial<ConfidenceServerProviderLocal> = {},
): ConfidenceServerProviderLocal {
  return {
    metadata: { name: 'ConfidenceServerProviderLocal' },
    resolve: vi.fn().mockResolvedValue({
      flags: { 'test-flag': { value: true, reason: 'MATCH' } },
      resolveToken: 'test-token',
      resolveId: 'test-resolve-id',
    }),
    applyFlag: vi.fn(),
    rulesOfHooks: 'once',
    resolveBooleanEvaluation: vi.fn(),
    resolveStringEvaluation: vi.fn(),
    resolveNumberEvaluation: vi.fn(),
    resolveObjectEvaluation: vi.fn(),
    ...overrides,
  } as unknown as ConfidenceServerProviderLocal;
}

// Mock provider that is NOT ConfidenceServerProviderLocal
function createMockOtherProvider(): Provider {
  return {
    metadata: { name: 'SomeOtherProvider' },
    rulesOfHooks: 'once',
    resolveBooleanEvaluation: vi.fn(),
    resolveStringEvaluation: vi.fn(),
    resolveNumberEvaluation: vi.fn(),
    resolveObjectEvaluation: vi.fn(),
  } as unknown as Provider;
}

describe('ConfidenceProvider', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(async () => {
    // Clean up OpenFeature state
    await OpenFeature.clearProviders();
  });

  describe('provider validation', () => {
    it('warns and returns children when default provider is not ConfidenceServerProviderLocal', async () => {
      const otherProvider = createMockOtherProvider();
      await OpenFeature.setProviderAndWait(otherProvider);
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

      // Should not throw, but should warn and return children
      const result = await ConfidenceProvider({
        context: { targetingKey: 'user-123' },
        children: <div>Test</div>,
      });

      expect(warnSpy).toHaveBeenCalledWith(
        expect.stringContaining('ConfidenceProvider requires a ConfidenceServerProviderLocal'),
      );
      expect(result).toBeDefined();
      warnSpy.mockRestore();
    });

    it('warns and returns children when named provider is not ConfidenceServerProviderLocal', async () => {
      const otherProvider = createMockOtherProvider();
      await OpenFeature.setProviderAndWait('my-provider', otherProvider);
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

      const result = await ConfidenceProvider({
        context: { targetingKey: 'user-123' },
        providerName: 'my-provider',
        children: <div>Test</div>,
      });

      expect(warnSpy).toHaveBeenCalledWith(
        expect.stringContaining('ConfidenceProvider requires a ConfidenceServerProviderLocal'),
      );
      expect(result).toBeDefined();
      warnSpy.mockRestore();
    });

    it('warns and returns children when no provider is registered', async () => {
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

      const result = await ConfidenceProvider({
        context: { targetingKey: 'user-123' },
        children: <div>Test</div>,
      });

      expect(warnSpy).toHaveBeenCalledWith(
        expect.stringContaining('ConfidenceProvider requires a ConfidenceServerProviderLocal'),
      );
      expect(result).toBeDefined();
      warnSpy.mockRestore();
    });

    it('identifies provider by metadata.name, not instanceof', async () => {
      const mockProvider = createMockConfidenceProvider();
      await OpenFeature.setProviderAndWait(mockProvider);

      // Should not throw - provider is identified by metadata name
      const result = await ConfidenceProvider({
        context: { targetingKey: 'user-123' },
        children: <div>Test</div>,
      });

      expect(result).toBeDefined();
    });
  });

  describe('flag resolution', () => {
    it('calls resolve with context and flags', async () => {
      const mockProvider = createMockConfidenceProvider();
      await OpenFeature.setProviderAndWait(mockProvider);

      const context: EvaluationContext = {
        targetingKey: 'user-123',
        country: 'US',
      };

      await ConfidenceProvider({
        context,
        flags: ['flag-a', 'flag-b'],
        children: <div>Test</div>,
      });

      expect(mockProvider.resolve).toHaveBeenCalledWith(context, ['flag-a', 'flag-b']);
    });

    it('calls resolve with empty flags array by default', async () => {
      const mockProvider = createMockConfidenceProvider();
      await OpenFeature.setProviderAndWait(mockProvider);

      await ConfidenceProvider({
        context: { targetingKey: 'user-123' },
        children: <div>Test</div>,
      });

      expect(mockProvider.resolve).toHaveBeenCalledWith({ targetingKey: 'user-123' }, []);
    });
  });

  describe('named providers', () => {
    it('uses named provider when providerName is specified', async () => {
      const defaultProvider = createMockConfidenceProvider();
      const namedProvider = createMockConfidenceProvider();
      await OpenFeature.setProviderAndWait(defaultProvider);
      await OpenFeature.setProviderAndWait('custom-provider', namedProvider);

      await ConfidenceProvider({
        context: { targetingKey: 'user-123' },
        providerName: 'custom-provider',
        children: <div>Test</div>,
      });

      expect(namedProvider.resolve).toHaveBeenCalled();
      expect(defaultProvider.resolve).not.toHaveBeenCalled();
    });

    it('uses default provider when providerName is not specified', async () => {
      const defaultProvider = createMockConfidenceProvider();
      const namedProvider = createMockConfidenceProvider();
      await OpenFeature.setProviderAndWait(defaultProvider);
      await OpenFeature.setProviderAndWait('custom-provider', namedProvider);

      await ConfidenceProvider({
        context: { targetingKey: 'user-123' },
        children: <div>Test</div>,
      });

      expect(defaultProvider.resolve).toHaveBeenCalled();
      expect(namedProvider.resolve).not.toHaveBeenCalled();
    });
  });
});
