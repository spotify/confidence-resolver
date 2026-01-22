# React Integration

React hooks and components for using Confidence feature flags in Next.js applications with React Server Components.

## Overview

This integration provides:

- **Server Component** (`ConfidenceProvider`) - Resolves flags on the server and provides them to client components
- **Client Hooks** (`useFlag`, `useFlagDetails`, `useFlagNames`) - Access flag values in client components with automatic exposure logging
- **Dot notation** - Access nested properties within flag values (e.g., `my-flag.config.enabled`)
- **Manual exposure control** - Delay exposure logging until user interaction
- **Full flag details** - Access variant, reason, and error information

## Installation

```bash
yarn add @spotify-confidence/openfeature-server-provider-local react
```

## Quick Start (Next.js App Router)

### 1. Set up the provider (server-side)

Create a file to initialize the OpenFeature provider:

```ts
// lib/confidence.ts
import { OpenFeature } from '@openfeature/server-sdk';
import { createConfidenceServerProvider } from '@spotify-confidence/openfeature-server-provider-local';

const provider = createConfidenceServerProvider({
  flagClientSecret: process.env.CONFIDENCE_FLAG_CLIENT_SECRET!,
});

// Initialize once at startup
OpenFeature.setProviderAndWait(provider);
```

### 2. Wrap your app with ConfidenceProvider

In your layout or page (Server Component):

```tsx
// app/layout.tsx
import { ConfidenceProvider } from '@spotify-confidence/openfeature-server-provider-local/react-server';
import './lib/confidence'; // Initialize provider

export default async function RootLayout({ children }: { children: React.ReactNode }) {
  // Get user context from session, cookies, etc.
  const evalContext = {
    targetingKey: 'user-123',
    country: 'US',
  };

  return (
    <html>
      <body>
        <ConfidenceProvider evalContext={evalContext}>{children}</ConfidenceProvider>
      </body>
    </html>
  );
}
```

### 3. Use flags in client components

```tsx
// components/FeatureButton.tsx
'use client';

import { useFlag } from '@spotify-confidence/openfeature-server-provider-local/react-client';

export function FeatureButton() {
  const enabled = useFlag('my-feature', false);

  if (!enabled) return null;

  return <button>New Feature</button>;
}
```

## API Reference

### ConfidenceProvider (Server Component)

Resolves flags on the server and provides them to client components via React Context.

```tsx
import { ConfidenceProvider } from '@spotify-confidence/openfeature-server-provider-local/react-server';

<ConfidenceProvider
  evalContext={{ targetingKey: 'user-123' }}
  flags={['feature-a', 'feature-b']} // Optional: specific flags to resolve
  providerName="my-provider" // Optional: if using named providers
>
  {children}
</ConfidenceProvider>;
```

**Props:**

| Prop           | Type                | Required | Description                                           |
| -------------- | ------------------- | -------- | ----------------------------------------------------- |
| `evalContext`  | `EvaluationContext` | Yes      | User/session context for flag evaluation              |
| `flags`        | `string[]`          | No       | Specific flags to resolve (default: all flags)        |
| `providerName` | `string`            | No       | Named provider if not using the default               |
| `children`     | `React.ReactNode`   | Yes      | Child components that will have access to flag values |

### useFlag (Client Hook)

Simple hook to get a flag value. Automatically logs exposure when the component mounts.

```tsx
import { useFlag } from '@spotify-confidence/openfeature-server-provider-local/react-client';

// Boolean flag
const enabled = useFlag('my-feature', false);

// String flag
const variant = useFlag('button-color', 'blue');

// Number flag
const limit = useFlag('max-items', 10);

// Object flag
const config = useFlag('feature-config', { enabled: false, limit: 0 });
```

**Dot Notation:**

Access nested properties within a flag value:

```tsx
// Flag value: { config: { maxItems: 10, enabled: true } }
const maxItems = useFlag('my-feature.config.maxItems', 5);
const enabled = useFlag('my-feature.config.enabled', false);
```

### useFlagDetails (Client Hook)

Hook that returns full flag details including variant, reason, and error information. Also supports manual exposure control.

```tsx
import { useFlagDetails } from '@spotify-confidence/openfeature-server-provider-local/react-client';

// Auto exposure (default) - logs exposure on mount
const { value, variant, reason } = useFlagDetails('my-feature', false);

// Manual exposure - log when user interacts
const { value: enabled, expose } = useFlagDetails('my-feature', false, { expose: false });

const handleClick = () => {
  if (enabled) {
    expose(); // Log exposure only when user clicks
    doSomething();
  }
};

// Check for errors
const { value, reason, errorCode } = useFlagDetails('my-feature', false);
if (errorCode === 'FLAG_NOT_FOUND') {
  console.warn('Flag not found, using default');
}
```

**Return Type:**

```ts
interface FlagDetails<T> {
  value: T; // The resolved flag value
  variant?: string; // The variant name (e.g., 'control', 'treatment')
  reason: string; // Resolution reason: 'MATCH', 'NO_MATCH', 'ERROR', etc.
  errorCode?: string; // Error code if resolution failed (e.g., 'FLAG_NOT_FOUND')
  errorMessage?: string; // Human-readable error message
  expose?: () => void; // Function to manually log exposure (only when { expose: false })
}
```

### useFlagNames (Client Hook)

Hook to get a list of all flag names available in the bundle.

```tsx
import { useFlagNames } from '@spotify-confidence/openfeature-server-provider-local/react-client';

const flagNames = useFlagNames();
// ['feature-a', 'feature-b', 'experiment-1']

// Useful for debugging or dynamic flag iteration
flagNames.forEach(name => {
  console.log(`Flag available: ${name}`);
});
```

Returns an empty array if no provider is present.

## Type Safety

The hooks validate that flag values match the type of your default value:

```tsx
// If the flag value is a string but you expect a number,
// the default value is returned instead
const limit = useFlag('my-flag', 10); // Returns 10 if flag value isn't a number

// Object structure is also validated
const config = useFlag('my-flag', { enabled: false, limit: 0 });
// Returns default if flag value doesn't have 'enabled' and 'limit' properties
```

## How It Works

1. **Server-side resolution**: `ConfidenceProvider` calls `resolveFlagBundle()` to resolve all requested flags in a single call
2. **Serialization**: The flag bundle (values + resolve token) is serialized and passed to the client
3. **Client hydration**: `ConfidenceClientProvider` receives the bundle and makes it available via React Context
4. **Exposure logging**: When `useFlag` is called, a Server Action sends the exposure event back to the server
5. **WASM processing**: The server uses the WASM resolver to process the exposure and batch it for sending to Confidence

## Best Practices

### Resolve flags at the layout level

Resolve flags once in a layout component rather than in each page:

```tsx
// app/(authenticated)/layout.tsx
export default async function AuthenticatedLayout({ children }) {
  const user = await getUser();

  return <ConfidenceProvider evalContext={{ targetingKey: user.id, plan: user.plan }}>{children}</ConfidenceProvider>;
}
```

### Use manual exposure for conditional features

When a feature is only shown after user interaction, use manual exposure to avoid logging exposures for users who never see the feature:

```tsx
const { value: showPromo, expose } = useFlagDetails('promo-banner', false, { expose: false });

const handleOpenModal = () => {
  if (showPromo) {
    expose(); // Only log when user actually opens the modal
    openPromoModal();
  }
};
```

### Specify flags for better performance

If you only need a few flags, specify them to reduce the bundle size:

```tsx
<ConfidenceProvider evalContext={context} flags={['feature-a', 'feature-b']}>
  {children}
</ConfidenceProvider>
```

## Troubleshooting

### "ConfidenceProvider requires a ConfidenceServerProviderLocal"

Make sure you've initialized the OpenFeature provider before rendering:

```ts
import { OpenFeature } from '@openfeature/server-sdk';
import { createConfidenceServerProvider } from '@spotify-confidence/openfeature-server-provider-local';

const provider = createConfidenceServerProvider({
  flagClientSecret: process.env.CONFIDENCE_FLAG_CLIENT_SECRET!,
});

await OpenFeature.setProviderAndWait(provider);
```

### "useFlagDetails called without a ConfidenceProvider"

This warning appears when `useFlag` or `useFlagDetails` is called outside of a `ConfidenceProvider`. Make sure your component tree is wrapped:

```tsx
<ConfidenceProvider evalContext={context}>
  <MyComponent /> {/* useFlag works here */}
</ConfidenceProvider>
```

### Flag value is always the default

Check that:

1. The flag exists in Confidence and is enabled
2. The targeting rules match your evaluation context
3. The flag value type matches your default value type (the hooks validate types)
