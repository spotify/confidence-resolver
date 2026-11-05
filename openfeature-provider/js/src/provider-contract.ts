import type { EvaluationContext, JsonValue, Provider } from '@openfeature/server-sdk';
import type FlagBundle from './flag-bundle';
import { ErrorCode } from './types';

/**
 * Evaluation details common to the state-based and module-based providers;
 * both return supersets of this shape (e.g. `shouldApply`,
 * `assignmentOrigin` on the state-based provider).
 */
export interface ConfidenceEvaluationDetails<T> {
  value: T;
  reason: string;
  variant?: string;
  errorCode?: ErrorCode;
  errorMessage?: string;
}

/**
 * The structural surface the React integrations (app router and pages
 * router) need from a Confidence provider: resolve flags into bundles with
 * deferred exposure, apply exposures later, and evaluate single flags with
 * immediate exposure.
 *
 * Implemented by `ConfidenceServerProviderLocal` (state-based) and
 * `ConfidenceModuleProvider` (compiled client module).
 */
export interface ConfidenceBundleProvider extends Provider {
  /** Resolve flags into a bundle without logging exposure. */
  resolve(context: EvaluationContext, flagNames: string[]): Promise<FlagBundle>;
  /** Log exposure for a previously resolved flag. */
  applyFlag(resolveToken: string, flagName: string): void;
  /** Evaluate one flag and log exposure immediately. */
  evaluate<T extends JsonValue>(
    flagKey: string,
    defaultValue: T,
    context: EvaluationContext,
  ): Promise<ConfidenceEvaluationDetails<T>>;
}

/**
 * Narrow an OpenFeature provider to the Confidence bundle API by structure
 * (capability) rather than by provider name, so every provider implementing
 * the surface — state-based or module-based — is accepted.
 */
export function isConfidenceProvider(provider: Provider): provider is ConfidenceBundleProvider {
  const candidate = provider as Partial<ConfidenceBundleProvider> | undefined;
  return (
    typeof candidate?.resolve === 'function' &&
    typeof candidate?.applyFlag === 'function' &&
    typeof candidate?.evaluate === 'function'
  );
}
