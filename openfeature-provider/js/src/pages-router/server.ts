import type { EvaluationContext } from '@openfeature/server-sdk';
import type { GetServerSideProps, GetServerSidePropsContext, GetServerSidePropsResult, Redirect } from 'next';
import * as FlagBundle from '../flag-bundle';
import { ErrorCode } from '../types';
import { getConfidenceProvider } from './common';
import { sealResolveToken } from './token';
import type { ConfidencePageProps } from './types';

export type { ConfidencePageProps } from './types';

export interface WithConfidenceOptions {
  /** Use a non-default OpenFeature provider by name. */
  providerName?: string;
}

/**
 * Return type of the function passed to `withConfidence`. Augments the standard
 * `getServerSideProps` shape with a required `context` (the evaluation context
 * for flag resolution) and an optional `flags` allow-list.
 */
export type WithConfidenceResult<P extends { [key: string]: any; confidence?: never }> =
  | {
      props: P | Promise<P>;
      /**
       * Evaluation context for flag resolution. Omit to skip flag resolution
       * entirely for this request — `pageProps.confidence` will be absent and
       * any `useFlag` / `useFlagDetails` calls in the tree fall back to
       * default values. Useful when flag access is conditional (e.g.
       * unauthenticated requests).
       */
      context?: EvaluationContext;
      /** Restrict resolution to a subset of flags. Defaults to all. */
      flags?: string[];
    }
  | { redirect: Redirect }
  | { notFound: true };

function providerMissingBundle(providerName?: string): ConfidencePageProps {
  return FlagBundle.error(
    ErrorCode.GENERAL,
    `OpenFeature provider${
      providerName ? ` "${providerName}"` : ''
    } is not a ConfidenceServerProviderLocal. Register one with OpenFeature.setProviderAndWait — typically in instrumentation.ts.`,
  );
}

/**
 * Low-level: resolve flags into a `ConfidencePageProps` payload. Use this if
 * you want flag resolution outside the `withConfidence` flow (e.g. inside a
 * custom decorator). Most callers should just use `withConfidence`.
 */
export async function resolveConfidence(
  context: EvaluationContext,
  opts: { flags?: string[]; providerName?: string } = {},
): Promise<ConfidencePageProps> {
  const provider = getConfidenceProvider(opts.providerName);
  if (!provider) return providerMissingBundle(opts.providerName);
  const bundle = await provider.resolve(context, opts.flags ?? []);
  // Replace the raw resolveToken in place with its sealed (AES-GCM) form.
  // The apply API route opens it; the client never sees the raw value.
  return { ...bundle, resolveToken: sealResolveToken(bundle.resolveToken) };
}

/**
 * `getServerSideProps` decorator. Pass a single function that does your data
 * fetching AND returns the evaluation context for flag resolution. The
 * decorator resolves the flag bundle, seals it, and merges it into
 * `pageProps.confidence`.
 *
 * Returning `{ redirect }` or `{ notFound }` short-circuits before flag
 * resolution, just like a normal `getServerSideProps`.
 *
 * @example
 * export const getServerSideProps = withConfidence(async ({ req }) => {
 *   const visitorId = req.cookies.uid ?? 'anon';
 *   const data = await fetchSomeData();
 *   return {
 *     props: { data },
 *     context: { visitor_id: visitorId },
 *   };
 * });
 */
export function withConfidence<P extends { [key: string]: any; confidence?: never } = {}>(
  gssp: (ctx: GetServerSidePropsContext) => Promise<WithConfidenceResult<P>>,
  opts: WithConfidenceOptions = {},
): GetServerSideProps<P & { confidence?: ConfidencePageProps }> {
  return async ctx => {
    const result = await gssp(ctx);
    if (!('props' in result)) {
      return result as GetServerSidePropsResult<P & { confidence?: ConfidencePageProps }>;
    }
    const innerProps = (await Promise.resolve(result.props)) as P;
    if (result.context === undefined) {
      return { props: innerProps };
    }
    const confidence = await resolveConfidence(result.context, {
      flags: result.flags,
      providerName: opts.providerName,
    });
    return { props: { ...innerProps, confidence } };
  };
}
