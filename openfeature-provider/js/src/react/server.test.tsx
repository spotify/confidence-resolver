/**
 * @vitest-environment happy-dom
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { OpenFeature } from '@openfeature/server-sdk';
import type { Provider, EvaluationContext } from '@openfeature/server-sdk';
import React from 'react';
import { ConfidenceProvider } from './server';
import type { ConfidenceBundleProvider } from '../provider-contract';

// Mock provider implementing the Confidence bundle API contract
function createMockConfidenceProvider(
  overrides: Partial<ConfidenceBundleProvider> = {},
  name = 'ConfidenceServerProviderLocal',
): ConfidenceBundleProvider {
  return {
    metadata: { name },
    resolve: vi.fn().mockResolvedValue({
      flags: { 'test-flag': { value: true, reason: 'MATCH' } },
      resolveToken: 'test-token',
      resolveId: 'test-resolve-id',
    }),
    applyFlag: vi.fn(),
    evaluate: vi.fn(),
    rulesOfHooks: 'once',
    resolveBooleanEvaluation: vi.fn(),
    resolveStringEvaluation: vi.fn(),
    resolveNumberEvaluation: vi.fn(),
    resolveObjectEvaluation: vi.fn(),
    ...overrides,
  } as unknown as ConfidenceBundleProvider;
}

// Mock provider that does NOT implement the Confidence bundle API
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
    it('warns and returns children when default provider does not implement the bundle API', async () => {
      const otherProvider = createMockOtherProvider();
      await OpenFeature.setProviderAndWait(otherProvider);
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

      // Should not throw, but should warn and return children
      const result = await ConfidenceProvider({
        context: { targetingKey: 'user-123' },
        children: <div>Test</div>,
      });

      expect(warnSpy).toHaveBeenCalledWith(
        expect.stringContaining('ConfidenceProvider requires a Confidence provider'),
      );
      expect(result).toBeDefined();
      warnSpy.mockRestore();
    });

    it('warns and returns children when named provider does not implement the bundle API', async () => {
      const otherProvider = createMockOtherProvider();
      await OpenFeature.setProviderAndWait('my-provider', otherProvider);
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

      const result = await ConfidenceProvider({
        context: { targetingKey: 'user-123' },
        providerName: 'my-provider',
        children: <div>Test</div>,
      });

      expect(warnSpy).toHaveBeenCalledWith(
        expect.stringContaining('ConfidenceProvider requires a Confidence provider'),
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
        expect.stringContaining('ConfidenceProvider requires a Confidence provider'),
      );
      expect(result).toBeDefined();
      warnSpy.mockRestore();
    });

    it('accepts any provider implementing the bundle API, regardless of name', async () => {
      const mockProvider = createMockConfidenceProvider({}, 'MyCustomConfidenceProvider');
      await OpenFeature.setProviderAndWait(mockProvider);
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

      const result = await ConfidenceProvider({
        context: { targetingKey: 'user-123' },
        children: <div>Test</div>,
      });

      // No warning, and the bundle API was used
      expect(warnSpy).not.toHaveBeenCalled();
      expect(mockProvider.resolve).toHaveBeenCalled();
      expect(result).toBeDefined();
      warnSpy.mockRestore();
    });

    it('rejects a provider by structure even when named like a Confidence provider', async () => {
      const fakeProvider = {
        metadata: { name: 'ConfidenceServerProviderLocal' },
        resolveBooleanEvaluation: vi.fn(),
      } as unknown as Provider;
      await OpenFeature.setProviderAndWait(fakeProvider);
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

      const result = await ConfidenceProvider({
        context: { targetingKey: 'user-123' },
        children: <div>Test</div>,
      });

      expect(warnSpy).toHaveBeenCalledWith(
        expect.stringContaining('ConfidenceProvider requires a Confidence provider'),
      );
      expect(result).toBeDefined();
      warnSpy.mockRestore();
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

  describe('resolve token is not exposed to client', () => {
    it('strips resolveToken from the bundle passed to ConfidenceClientProvider', async () => {
      const mockProvider = createMockConfidenceProvider();
      await OpenFeature.setProviderAndWait(mockProvider);

      const result = await ConfidenceProvider({
        context: { targetingKey: 'user-123' },
        children: <div>Test</div>,
      });

      // The bundle prop ships to the browser via the RSC payload — make sure
      // the resolve token (which carries the full evaluation context and the
      // resolved variants) does not.
      const clientProps = (result as React.ReactElement<{ bundle: { resolveToken: string } }>).props;
      expect(clientProps.bundle.resolveToken).toBe('');
    });

    it('still applies flags using the original resolve token via the server action closure', async () => {
      const mockProvider = createMockConfidenceProvider();
      await OpenFeature.setProviderAndWait(mockProvider);

      const result = await ConfidenceProvider({
        context: { targetingKey: 'user-123' },
        children: <div>Test</div>,
      });

      const clientProps = (result as React.ReactElement<{ apply: (flagName: string) => Promise<void> }>).props;
      await clientProps.apply('test-flag');

      expect(mockProvider.applyFlag).toHaveBeenCalledWith('test-token', 'test-flag');
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
