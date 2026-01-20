'use client';
import React, { createContext, useMemo } from 'react';
import type { FlagBundle, ApplyFn } from './types';

export interface ConfidenceContextValue {
  bundle: FlagBundle;
  apply: ApplyFn;
}

export const ConfidenceContext: React.Context<ConfidenceContextValue | null> =
  createContext<ConfidenceContextValue | null>(null);

export interface ConfidenceProviderProps {
  bundle: FlagBundle;
  apply: ApplyFn;
  children: React.ReactNode;
}

/**
 * React context provider for Confidence feature flags.
 * Provides pre-resolved flag values to client components without bundling the WASM resolver.
 *
 * @example
 * ```tsx
 * // In a server component
 * const { bundle, applyFlag } = await provider.createFlagBundle({ targetingKey: 'user-123' });
 *
 * // Wrap in ConfidenceProvider
 * <ConfidenceProvider bundle={bundle} apply={applyFlag}>
 *   {children}
 * </ConfidenceProvider>
 * ```
 */
export function ConfidenceProvider({ bundle, apply, children }: ConfidenceProviderProps): React.ReactNode {
  const value = useMemo(() => ({ bundle, apply }), [bundle, apply]);
  return <ConfidenceContext.Provider value={value}>{children}</ConfidenceContext.Provider>;
}
