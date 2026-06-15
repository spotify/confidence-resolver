declare const window: { __confidence?: { flags?: Record<string, { variant: string }> } } | undefined;

export function publishFlagEvaluation(name: string, variant: string): void {
  if (typeof window === 'undefined') return;

  const confidence = (window.__confidence ??= {});
  const flags = (confidence.flags ??= {});

  if (flags[name]?.variant === variant) return;

  flags[name] = { variant };
}
