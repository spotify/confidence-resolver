'use client';

import { createContext, useContext, useEffect, useRef } from 'react';
import type { FlagBundle, ResolvedFlagDetails } from './types';

type ApplyFn = (flagName: string) => Promise<void>;

interface ConfidenceContextValue {
  bundle: FlagBundle;
  apply: ApplyFn;
}

const ConfidenceContext = createContext<ConfidenceContextValue | null>(null);

export interface ConfidenceClientProviderProps {
  bundle: FlagBundle;
  apply: ApplyFn;
  children: React.ReactNode;
}

export function ConfidenceClientProvider({
  bundle,
  apply,
  children,
}: ConfidenceClientProviderProps): React.ReactElement {
  return <ConfidenceContext.Provider value={{ bundle, apply }}>{children}</ConfidenceContext.Provider>;
}

function useConfidenceContext(): ConfidenceContextValue {
  const context = useContext(ConfidenceContext);
  if (!context) {
    throw new Error('useFlag must be used within a ConfidenceProvider');
  }
  return context;
}

export interface UseFlagOptions {
  /** If true, the flag will be automatically applied/exposed when accessed. Default: true */
  expose?: boolean;
}

/**
 * Hook to get a flag value with automatic exposure tracking.
 * Use this for simple flag access where you always want to track exposure.
 */
export function useFlag<T>(flagName: string, defaultValue: T): T {
  const { bundle, apply } = useConfidenceContext();
  const appliedRef = useRef(false);

  useEffect(() => {
    if (!appliedRef.current) {
      appliedRef.current = true;
      apply(flagName);
    }
  }, [flagName, apply]);

  const details = bundle.flags[flagName];
  if (!details || details.reason !== 'MATCH') {
    return defaultValue;
  }

  return details.value as T;
}

/**
 * Hook to get flag details with optional manual exposure control.
 * Use this when you need access to variant, reason, or manual exposure tracking.
 */
export function useFlagDetails<T>(
  flagName: string,
  defaultValue: T,
  options: UseFlagOptions = {},
): ResolvedFlagDetails & { value: T; reportExposure: () => void } {
  const { bundle, apply } = useConfidenceContext();
  const appliedRef = useRef(false);
  const { expose = true } = options;

  const reportExposure = () => {
    if (!appliedRef.current) {
      appliedRef.current = true;
      apply(flagName);
    }
  };

  useEffect(() => {
    if (expose && !appliedRef.current) {
      appliedRef.current = true;
      apply(flagName);
    }
  }, [flagName, apply, expose]);

  const details = bundle.flags[flagName];
  if (!details || details.reason !== 'MATCH') {
    return {
      value: defaultValue,
      reason: details?.reason ?? 'ERROR',
      errorCode: details?.errorCode ?? 'FLAG_NOT_FOUND',
      errorMessage: details?.errorMessage,
      reportExposure,
    };
  }

  return {
    value: details.value as T,
    variant: details.variant,
    reason: details.reason,
    reportExposure,
  };
}
