'use client';

import { useCallback, type ReactNode } from 'react';
// Package self-reference so the pages-router/client bundle does NOT inline
// react/client — otherwise we'd ship two copies of `ConfidenceContext` and
// `<ConfidencePagesProvider>` would write to a different context than the one
// `useFlag` / `useFlagDetails` read from. The consumer's bundler resolves
// this via the package's `./react-client` exports entry and dedupes it against
// any other usage of react-client in the same app.
import { ConfidenceClientProvider } from '@spotify-confidence/openfeature-server-provider-local/react-client';
import type { ConfidencePageProps } from './types';

const DEFAULT_APPLY_PATH = '/api/confidence/apply';

export type { ConfidencePageProps } from './types';

interface Props {
  /**
   * The Confidence payload returned from `withConfidence` in
   * `getServerSideProps`, normally pulled out of `pageProps` in `_app.tsx`.
   * Pages that don't resolve flags pass `undefined` and any `useFlag` /
   * `useFlagDetails` calls in their tree return defaults.
   */
  confidence?: ConfidencePageProps;
  /** Override the apply API route. Must match where you mount `applyHandler`. */
  apiPath?: string;
  children: ReactNode;
}

/**
 * Place at the top of `_app.tsx`. Bridges the bundle resolved on the server
 * to the client `useFlag` / `useFlagDetails` hooks.
 */
export function ConfidencePagesProvider({
  confidence,
  apiPath = DEFAULT_APPLY_PATH,
  children,
}: Props): React.ReactElement {
  const resolveToken = confidence?.resolveToken;

  const apply = useCallback(
    async (flagName: string) => {
      if (!resolveToken) return;
      try {
        const res = await fetch(apiPath, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ resolveToken, flagName }),
          keepalive: true,
        });
        if (!res.ok && process.env.NODE_ENV !== 'production') {
          // eslint-disable-next-line no-console
          console.warn(
            `[Confidence] apply failed: ${res.status} ${res.statusText}. ` +
              `Mount applyHandler at ${apiPath} and ensure a ConfidenceServerProviderLocal is registered.`,
          );
        }
      } catch (err) {
        if (process.env.NODE_ENV !== 'production') {
          // eslint-disable-next-line no-console
          console.warn('[Confidence] apply request failed:', err);
        }
      }
    },
    [resolveToken, apiPath],
  );

  if (!confidence) return <>{children}</>;
  return (
    <ConfidenceClientProvider bundle={confidence} apply={apply}>
      {children}
    </ConfidenceClientProvider>
  );
}
