import type { EvaluationContext } from '@openfeature/server-sdk';
import type { GetServerSideProps, GetServerSidePropsContext, GetServerSidePropsResult } from 'next';
import * as FlagBundle from '../flag-bundle';
import { ErrorCode } from '../types';
import { getConfidenceProvider } from './common';
import { sealResolveToken } from './token';
import type { ConfidencePageProps } from './types';

export type { ConfidencePageProps } from './types';

export interface WithConfidenceConfig {
  /**
   * Evaluation context for flag resolution. Either a static value or a function
   * that derives one from the request. Mirrors `<ConfidenceProvider context>`
   * from the App Router integration.
   */
  context: EvaluationContext | ((ctx: GetServerSidePropsContext) => EvaluationContext | Promise<EvaluationContext>);
  /** Restrict resolution to a subset of flags. Defaults to all. */
  flags?: string[];
  /** Use a non-default OpenFeature provider by name. */
  providerName?: string;
}

/**
 * Low-level: resolve flags into a `ConfidencePageProps` payload. Most callers
 * should use `withConfidence` instead.
 */
export async function resolveConfidence(
  config: WithConfidenceConfig,
  ctx: GetServerSidePropsContext,
): Promise<ConfidencePageProps> {
  const provider = getConfidenceProvider(config.providerName);
  if (!provider) {
    return FlagBundle.error(
      ErrorCode.GENERAL,
      `OpenFeature provider${
        config.providerName ? ` "${config.providerName}"` : ''
      } is not a ConfidenceServerProviderLocal. Register one with OpenFeature.setProviderAndWait — typically in instrumentation.ts.`,
    );
  }

  const context = typeof config.context === 'function' ? await config.context(ctx) : config.context;
  const bundle = await provider.resolve(context, config.flags ?? []);
  // Replace the raw resolveToken in place with its sealed (AES-GCM) form.
  // The apply API route opens it; the client never sees the raw value.
  return { ...bundle, resolveToken: sealResolveToken(bundle.resolveToken) };
}

/**
 * Resolves flags and merges `confidence` into `props`. Composes with a user
 * gSSP that returns its own `props` / `redirect` / `notFound`. Pages without
 * their own data fetching call it without a second argument.
 */
export function withConfidence<P extends Record<string, unknown> = {}>(
  config: WithConfidenceConfig,
  inner?: GetServerSideProps<P>,
): GetServerSideProps<P & { confidence?: ConfidencePageProps }> {
  return async ctx => {
    const innerResult: GetServerSidePropsResult<P> = inner ? await inner(ctx) : { props: {} as P };

    if ('redirect' in innerResult || 'notFound' in innerResult) {
      return innerResult as GetServerSidePropsResult<P>;
    }

    const innerProps = (await Promise.resolve(innerResult.props)) as P;
    const confidence = await resolveConfidence(config, ctx);
    return { ...innerResult, props: { ...innerProps, confidence } };
  };
}
