# Next.js Integration Guide

This guide covers integrating the Confidence OpenFeature provider with Next.js App Router using React Server Components.

## Overview

A lightweight React module (`~1KB`) is available that consumes pre-resolved flags without bundling the WASM resolver (`~110KB`). Flags are resolved on the server and passed to client components via React Context.

## Installation

```bash
yarn add @spotify-confidence/openfeature-server-provider-local react
```

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
import type { FlagBundle, ApplyFn } from '@spotify-confidence/openfeature-server-provider-local/react';

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
  return provider.createFlagBundle(context);
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
import { ConfidenceProvider } from '@spotify-confidence/openfeature-server-provider-local/react';
import { MyClientComponent } from '@/components/MyClientComponent';

export default async function Page() {
  await initializeConfidence();

  const context = { visitor_id: 'user-123' };
  const { bundle } = await getBundle(context);

  // Bind the server action with the resolveToken
  const apply = applyFlag.bind(null, bundle.resolveToken);

  return (
    <ConfidenceProvider bundle={bundle} apply={apply}>
      <MyClientComponent />
    </ConfidenceProvider>
  );
}
```

---

## Client Components

### Auto Exposure

By default, `useFlag` automatically logs exposure when the component mounts:

```tsx
'use client';

import { useFlag } from '@spotify-confidence/openfeature-server-provider-local/react';

export function FeatureButton() {
  const enabled = useFlag('my-feature.enabled', false);
  return enabled ? <NewButton /> : <OldButton />;
}
```

### Manual Exposure

For cases where you want to control when exposure is logged (e.g., only when a user interacts with a feature):

```tsx
'use client';

import { useFlag } from '@spotify-confidence/openfeature-server-provider-local/react';

export function Checkout() {
  const { value: discountEnabled, expose } = useFlag('checkout.discount', false, { skipExposure: true });

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

### `createFlagBundle(context, flags?)`

Creates a bundle of pre-resolved flag values for client-side consumption.

- `context`: Evaluation context with `targetingKey` and optional attributes
- `flags` (optional): Array of specific flag names to resolve. If omitted, resolves all flags.
- Returns: `{ bundle, applyFlag }` where `applyFlag` is an async function pre-bound to the resolve token

### `useFlag(flagName, defaultValue, options?)`

React hook for accessing flag values.

- `flagName`: Name of the flag to access
- `defaultValue`: Default value if flag is not found
- `options.skipExposure`: When `true`, returns `{ value, expose }` for manual exposure control
