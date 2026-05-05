import type { NextApiHandler } from 'next';
import { getConfidenceProvider } from './common';
import { openResolveToken } from './token';

export interface ApplyHandlerOptions {
  /** Use a non-default OpenFeature provider by name. */
  providerName?: string;
}

/**
 * Builds the POST handler that the client `ConfidencePagesProvider` calls to
 * fire exposure events. Mount it at `/api/confidence/apply` (or pass a custom
 * `apiPath` to `ConfidencePagesProvider` if you mount it elsewhere).
 *
 * @example
 * // pages/api/confidence/apply.ts
 * import { applyHandler } from '@spotify-confidence/openfeature-server-provider-local/pages-router/api';
 * export default applyHandler();
 */
export function applyHandler(opts: ApplyHandlerOptions = {}): NextApiHandler {
  return async (req, res) => {
    if (req.method !== 'POST') {
      res.setHeader('Allow', 'POST');
      res.status(405).end();
      return;
    }

    const body = req.body as { resolveToken?: unknown; flagName?: unknown } | undefined;
    if (!body || typeof body.resolveToken !== 'string' || typeof body.flagName !== 'string') {
      res.status(400).end();
      return;
    }

    const provider = getConfidenceProvider(opts.providerName);
    if (!provider) {
      res.status(503).end();
      return;
    }

    let openedToken: string;
    try {
      openedToken = openResolveToken(body.resolveToken);
    } catch {
      res.status(400).end();
      return;
    }

    // Error bundles carry an empty resolveToken (see FlagBundle.error); skip
    // applyFlag in that case to match the App Router's `!bundle.errorCode` gate.
    if (!openedToken) {
      res.status(204).end();
      return;
    }

    provider.applyFlag(openedToken, body.flagName);
    res.status(204).end();
  };
}
