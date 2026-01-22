# @spotify-confidence/react

React hooks and components for Confidence feature flags.

## Overview

A lightweight React module (`~1KB`) that consumes pre-resolved flags without bundling the WASM resolver (`~110KB`). Flags are resolved on the server and passed to client components via React Context.

## Installation

```bash
# Client-side React package
yarn add @spotify-confidence/react

# Server-side provider (installed separately, typically in your backend or Next.js server code)
yarn add @spotify-confidence/openfeature-server-provider-local
```

Note: The packages are independent and have no runtime dependency on each other. The server-provider resolves flags and creates bundles that are passed to the React client components.

## Next.js 16 Configuration

Next.js 16 uses Turbopack by default. Due to a [known Turbopack limitation](https://github.com/vercel/next.js/issues/69159), `"use client"` directives in external packages are not recognized. Choose one of the following options:

### Option 1: Use Webpack (Recommended)

Switch to webpack by adding the `--webpack` flag:

```json
{
  "scripts": {
    "dev": "next dev --webpack",
    "build": "next build --webpack"
  }
}
```

### Option 2: Turbopack Workaround

If you need to use Turbopack, create local wrapper components that re-export the SDK's React components with a local `"use client"` directive:

```tsx
// app/components/ConfidenceWrapper.tsx
'use client';

import React, { createContext, useContext, useEffect, useMemo, useRef } from 'react';
import type { FlagBundle, ApplyFn } from '@spotify-confidence/react';

interface ConfidenceContextValue {
  bundle: FlagBundle;
  apply: ApplyFn;
}

const ConfidenceContext = createContext<ConfidenceContextValue | null>(null);

interface ConfidenceProviderProps {
  bundle: FlagBundle;
  apply: ApplyFn;
  children: React.ReactNode;
}

export function ConfidenceProvider({ bundle, apply, children }: ConfidenceProviderProps) {
  const value = useMemo(() => ({ bundle, apply }), [bundle, apply]);
  return <ConfidenceContext.Provider value={value}>{children}</ConfidenceContext.Provider>;
}

export function useFlag<T>(flagName: string, defaultValue: T): T {
  const ctx = useContext(ConfidenceContext);
  const appliedRef = useRef(false);

  useEffect(() => {
    if (ctx && !appliedRef.current) {
      appliedRef.current = true;
      ctx.apply(flagName);
    }
  }, [ctx, flagName]);

  return (ctx?.bundle.flags[flagName]?.value as T) ?? defaultValue;
}
```

Then import from this local file instead of the package:

```tsx
import { ConfidenceProvider, useFlag } from '@/components/ConfidenceWrapper';
```

---

## Server-Side Setup

```tsx
// app/lib/confidence.ts
import { createConfidenceServerProvider } from '@spotify-confidence/openfeature-server-provider-local';

let provider: ReturnType<typeof createConfidenceServerProvider> | null = null;

export async function initializeConfidence() {
  if (!provider) {
    provider = createConfidenceServerProvider({
      flagClientSecret: process.env.CONFIDENCE_FLAG_CLIENT_SECRET!,
      materializationStore: 'CONFIDENCE_REMOTE_STORE',
    });
  }
  return provider.getOpenFeatureClient();
}

export function getProvider() {
  if (!provider) throw new Error('Provider not initialized');
  return provider;
}

export async function getBundle(context: Record<string, unknown>) {
  if (!provider) throw new Error('Provider not initialized');
  return provider.resolveFlagBundle(context);
}
```

```tsx
// app/lib/actions.ts - Server action for apply
'use server';

import { initializeConfidence, getProvider } from './confidence';

export async function applyFlag(resolveToken: string, flagName: string): Promise<void> {
  await initializeConfidence();
  getProvider().applyFlag(resolveToken, flagName);
}
```

---

## Server Component

```tsx
// app/page.tsx
import { initializeConfidence, getBundle } from '@/lib/confidence';
import { applyFlag } from '@/lib/actions';
import { ConfidenceProvider } from '@spotify-confidence/react';
import { MyClientComponent } from '@/components/MyClientComponent';

export default async function Page() {
  await initializeConfidence();

  const context = { visitor_id: 'user-123' };
  const { bundle } = await getBundle(context);

  // Closure-based server action (resolveToken is encrypted)
  async function apply(flagName: string) {
    'use server';
    await applyFlag(bundle.resolveToken, flagName);
  }

  return (
    <ConfidenceProvider bundle={bundle} apply={apply}>
      <MyClientComponent />
    </ConfidenceProvider>
  );
}
```

### Server Action Security

Next.js automatically encrypts closed-over variables in server actions. The example above uses a closure so that `resolveToken` is encrypted when sent to the client.

**Avoid using `.bind()`** for sensitive values:

```tsx
// ⚠️ Values passed via .bind() are NOT encrypted
const apply = applyFlag.bind(null, bundle.resolveToken);

// ✅ Closed-over values ARE encrypted
async function apply(flagName: string) {
  'use server';
  await applyFlag(bundle.resolveToken, flagName);
}
```

**Multi-server deployments:** Next.js generates a new encryption key per build. When self-hosting across multiple servers, set `NEXT_SERVER_ACTIONS_ENCRYPTION_KEY` to ensure all instances use the same key.

For more details, see the [Next.js Security Guide](https://nextjs.org/docs/app/guides/data-security).

---

## Client Components

### Auto Exposure

By default, `useFlag` automatically logs exposure when the component mounts:

```tsx
'use client';

import { useFlag } from '@spotify-confidence/react';

export function FeatureButton() {
  const enabled = useFlag('my-feature.enabled', false);
  return enabled ? <NewButton /> : <OldButton />;
}
```

### Manual Exposure

For cases where you want to control when exposure is logged (e.g., only when a user interacts with a feature):

```tsx
'use client';

import { useFlagDetails } from '@spotify-confidence/react';

export function Checkout() {
  const { value: discountEnabled, expose } = useFlagDetails('checkout.discount', false, { expose: false });

  const handlePurchase = () => {
    if (discountEnabled) {
      expose(); // Log exposure only when user interacts
      applyDiscount();
    }
    completePurchase();
  };

  return <button onClick={handlePurchase}>Buy Now</button>;
}
```

---

## API Reference

### `resolveFlagBundle(context, flags?)`

Creates a bundle of pre-resolved flag values for client-side consumption.

- `context`: Evaluation context with `targetingKey` and optional attributes
- `flags` (optional): Array of specific flag names to resolve. If omitted, resolves all flags.
- Returns: `{ bundle, applyFlag }` where `applyFlag` is an async function pre-bound to the resolve token

### `useFlag(flagName, defaultValue)`

React hook for accessing flag values. Automatically logs exposure on mount.

- `flagName`: Name of the flag to access (supports dot notation for nested values)
- `defaultValue`: Default value if flag is not found or type doesn't match
- Returns: The flag value

### `useFlagDetails(flagName, defaultValue, options?)`

React hook for accessing flag values with optional manual exposure control.

- `flagName`: Name of the flag to access (supports dot notation for nested values)
- `defaultValue`: Default value if flag is not found or type doesn't match
- `options.expose`: Set to `false` for manual exposure control
- Returns: `{ value, expose? }` where `expose` is a function when using manual mode
