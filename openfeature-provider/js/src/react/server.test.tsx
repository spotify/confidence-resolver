/**
 * @vitest-environment happy-dom
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { OpenFeature } from '@openfeature/server-sdk';
import type { Provider, EvaluationContext } from '@openfeature/server-sdk';
import React from 'react';
import { ConfidenceProvider } from './server';

// Mock provider that matches ConfidenceServerProviderLocal's metadata
function createMockConfidenceProvider(overrides: Partial<Provider> = {}): Provider {
  return {
    metadata: { name: 'ConfidenceServerProviderLocal' },
    resolveFlagBundle: vi.fn().mockResolvedValue({
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
  } as unknown as Provider;
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
    it('throws error when default provider is not ConfidenceServerProviderLocal', async () => {
      const otherProvider = createMockOtherProvider();
      await OpenFeature.setProviderAndWait(otherProvider);

      await expect(
        ConfidenceProvider({
          evalContext: { targetingKey: 'user-123' },
          children: <div>Test</div>,
        }),
      ).rejects.toThrow('ConfidenceProvider requires a ConfidenceServerProviderLocal, but got SomeOtherProvider');
    });

    it('throws error when named provider is not ConfidenceServerProviderLocal', async () => {
      const otherProvider = createMockOtherProvider();
      await OpenFeature.setProviderAndWait('my-provider', otherProvider);

      await expect(
        ConfidenceProvider({
          evalContext: { targetingKey: 'user-123' },
          providerName: 'my-provider',
          children: <div>Test</div>,
        }),
      ).rejects.toThrow('ConfidenceProvider requires a ConfidenceServerProviderLocal, but got SomeOtherProvider');
    });

    it('throws error when no provider is registered', async () => {
      await expect(
        ConfidenceProvider({
          evalContext: { targetingKey: 'user-123' },
          children: <div>Test</div>,
        }),
      ).rejects.toThrow('ConfidenceProvider requires a ConfidenceServerProviderLocal');
    });

    it('identifies provider by metadata.name, not instanceof', async () => {
      const mockProvider = createMockConfidenceProvider();
      await OpenFeature.setProviderAndWait(mockProvider);

      // Should not throw - provider is identified by metadata name
      const result = await ConfidenceProvider({
        evalContext: { targetingKey: 'user-123' },
        children: <div>Test</div>,
      });

      expect(result).toBeDefined();
    });
  });

  describe('flag resolution', () => {
    it('calls resolveFlagBundle with evalContext and flags', async () => {
      const mockProvider = createMockConfidenceProvider();
      await OpenFeature.setProviderAndWait(mockProvider);

      const evalContext: EvaluationContext = {
        targetingKey: 'user-123',
        country: 'US',
      };

      await ConfidenceProvider({
        evalContext,
        flags: ['flag-a', 'flag-b'],
        children: <div>Test</div>,
      });

      expect(mockProvider.resolveFlagBundle).toHaveBeenCalledWith(evalContext, 'flag-a', 'flag-b');
    });

    it('calls resolveFlagBundle with empty flags array by default', async () => {
      const mockProvider = createMockConfidenceProvider();
      await OpenFeature.setProviderAndWait(mockProvider);

      await ConfidenceProvider({
        evalContext: { targetingKey: 'user-123' },
        children: <div>Test</div>,
      });

      expect(mockProvider.resolveFlagBundle).toHaveBeenCalledWith({ targetingKey: 'user-123' });
    });
  });

  describe('named providers', () => {
    it('uses named provider when providerName is specified', async () => {
      const defaultProvider = createMockConfidenceProvider();
      const namedProvider = createMockConfidenceProvider();
      await OpenFeature.setProviderAndWait(defaultProvider);
      await OpenFeature.setProviderAndWait('custom-provider', namedProvider);

      await ConfidenceProvider({
        evalContext: { targetingKey: 'user-123' },
        providerName: 'custom-provider',
        children: <div>Test</div>,
      });

      expect(namedProvider.resolveFlagBundle).toHaveBeenCalled();
      expect(defaultProvider.resolveFlagBundle).not.toHaveBeenCalled();
    });

    it('uses default provider when providerName is not specified', async () => {
      const defaultProvider = createMockConfidenceProvider();
      const namedProvider = createMockConfidenceProvider();
      await OpenFeature.setProviderAndWait(defaultProvider);
      await OpenFeature.setProviderAndWait('custom-provider', namedProvider);

      await ConfidenceProvider({
        evalContext: { targetingKey: 'user-123' },
        children: <div>Test</div>,
      });

      expect(defaultProvider.resolveFlagBundle).toHaveBeenCalled();
      expect(namedProvider.resolveFlagBundle).not.toHaveBeenCalled();
    });
  });
});
