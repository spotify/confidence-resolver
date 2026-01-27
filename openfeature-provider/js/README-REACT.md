# React Integration

React hooks and components for using Confidence feature flags in a modern React (RSC) App, like for instance Next.js.

## Overview

This integration provides:

- **Server Component** (`ConfidenceProvider`) - Resolves flags on the server and provides them to client components
- **Server Hooks** (`useFlag`, `useFlagDetails`) - Evaluate flags directly in server components with immediate exposure
- **Client Hooks** (`useFlag`, `useFlagDetails`) - Access flag values in client components with automatic or manual exposure logging
- **Dot notation** - Access properties within flag values (e.g., `my-flag.enabled`, `my-flag.config.limit`)
- **Manual exposure control** - Delay exposure logging until user interaction (client-side only)
- **Full flag details** - Access variant, reason, and error information

## Flag Structure

Confidence flags are always structured objects containing one or more properties. Use dot notation to access specific values:

```tsx
// Flag "checkout-flow" with value: { enabled: true, maxRetries: 3, theme: "dark" }
const enabled = useFlag('checkout-flow.enabled', false);
const maxRetries = useFlag('checkout-flow.maxRetries', 1);
const theme = useFlag('checkout-flow.theme', 'light');

// Or get the entire flag object
const checkoutConfig = useFlag('checkout-flow', { enabled: false, maxRetries: 1, theme: 'light' });
```

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
await OpenFeature.setProviderAndWait(provider);
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
  // Access the 'enabled' property of the 'new-feature' flag
  const enabled = useFlag('new-feature.enabled', false);

  if (!enabled) return null;

  return <button>New Feature</button>;
}
```

## Server vs Client: Understanding Exposure

**Exposure** is the event that tells Confidence a user was shown a particular flag variant. This is critical for accurate experiment analysis.

### Server-Side Exposure

When using `useFlag` or `useFlagDetails` from `react-server`, exposure is logged **immediately** when the flag is evaluated:

```tsx
// app/page.tsx (Server Component)
import { useFlag } from '@spotify-confidence/openfeature-server-provider-local/react-server';

export default async function Page() {
  // Exposure is logged immediately when this evaluates
  const showNewLayout = await useFlag('page-layout.showNewLayout', false, { targetingKey: 'user-123' });

  return showNewLayout ? <NewLayout /> : <OldLayout />;
}
```

This is appropriate for server components because:

- The component only renders once (no hydration)
- If the flag value affects what's rendered, the user will see it
- There's no concept of "mounting" - evaluation equals exposure

### Client-Side Exposure

When using hooks from `react-client`, you have two options:

#### Automatic Exposure (Default)

Exposure is logged when the component **mounts** (via `useEffect`):

```tsx
'use client';
import { useFlag } from '@spotify-confidence/openfeature-server-provider-local/react-client';

function MyComponent() {
  // Exposure logged on mount
  const enabled = useFlag('my-feature.enabled', false);
  return enabled ? <Feature /> : null;
}
```

#### Manual Exposure

Use `{ expose: false }` to control exactly when exposure is logged:

```tsx
'use client';
import { useFlagDetails } from '@spotify-confidence/openfeature-server-provider-local/react-client';

function MyComponent() {
  // No exposure logged automatically
  const { value: showPromo, expose } = useFlagDetails('promo-banner.show', false, { expose: false });

  const handleClick = () => {
    if (showPromo) {
      expose(); // Log exposure only when user clicks
      openPromoModal();
    }
  };

  return <button onClick={handleClick}>Open Promo</button>;
}
```

Manual exposure is useful when:

- A feature is only shown after user interaction
- You want to avoid counting users who never actually see the feature
- The flag controls something that may not be immediately visible

## API Reference

### Server Components

#### ConfidenceProvider

Resolves flags on the server and provides them to client components via React Context.

```tsx
import { ConfidenceProvider } from '@spotify-confidence/openfeature-server-provider-local/react-server';

<ConfidenceProvider
  evalContext={{ targetingKey: 'user-123' }}
  flags={['checkout-flow', 'promo-banner']} // Optional: specific flags to resolve
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

#### useFlag (Server)

Evaluate a flag directly in a server component. Logs exposure immediately.

```tsx
import { useFlag } from '@spotify-confidence/openfeature-server-provider-local/react-server';

// In an async Server Component - access a specific property
const enabled = await useFlag('checkout-flow.enabled', false, { targetingKey: 'user-123' });

// Or get the entire flag object
const config = await useFlag('checkout-flow', { enabled: false, maxRetries: 1 }, { targetingKey: 'user-123' });
```

**Parameters:**

| Parameter      | Type                | Required | Description                              |
| -------------- | ------------------- | -------- | ---------------------------------------- |
| `flagKey`      | `string`            | Yes      | The flag key (supports dot notation)     |
| `defaultValue` | `T`                 | Yes      | Default value if flag is not found       |
| `context`      | `EvaluationContext` | Yes      | User/session context for flag evaluation |
| `providerName` | `string`            | No       | Named provider if not using the default  |

#### useFlagDetails (Server)

Get full flag details in a server component. Logs exposure immediately.

```tsx
import { useFlagDetails } from '@spotify-confidence/openfeature-server-provider-local/react-server';

const { value, variant, reason } = await useFlagDetails('checkout-flow.enabled', false, { targetingKey: 'user-123' });
```

### Client Components

#### useFlag (Client)

Simple hook to get a flag value. Automatically logs exposure when the component mounts.

```tsx
import { useFlag } from '@spotify-confidence/openfeature-server-provider-local/react-client';

// Boolean property
const enabled = useFlag('my-feature.enabled', false);

// String property
const buttonColor = useFlag('ui-theme.buttonColor', 'blue');

// Number property
const maxItems = useFlag('pagination.limit', 10);

// Nested property
const retryLimit = useFlag('api-config.retry.maxAttempts', 3);

// Entire flag object
const config = useFlag('my-feature', { enabled: false, limit: 0 });
```

#### useFlagDetails (Client)

Hook that returns full flag details including variant, reason, and error information. Also supports manual exposure control.

```tsx
import { useFlagDetails } from '@spotify-confidence/openfeature-server-provider-local/react-client';

// Auto exposure (default) - logs exposure on mount
const { value, variant, reason } = useFlagDetails('checkout-flow.enabled', false);

// Manual exposure - log when user interacts
const { value: showBanner, expose } = useFlagDetails('promo-banner.show', false, { expose: false });

const handleClick = () => {
  if (showBanner) {
    expose(); // Log exposure only when user clicks
    doSomething();
  }
};

// Check for errors
const { value, reason, errorCode } = useFlagDetails('my-feature.enabled', false);
if (errorCode === 'FLAG_NOT_FOUND') {
  console.warn('Flag not found, using default');
}
```

**Return Type:**

```ts
interface ClientEvaluationDetails<T> extends EvaluationDetails<T> {
  expose: () => void; // Function to manually log exposure (no-op if auto-exposure is enabled)
}

// EvaluationDetails from @openfeature/core
interface EvaluationDetails<T> {
  flagKey: string; // The flag key that was requested
  flagMetadata: {}; // Reserved for future use
  value: T; // The resolved flag value
  variant?: string; // The variant name (e.g., 'control', 'treatment')
  reason: string; // Resolution reason: 'MATCH', 'NO_SEGMENT_MATCH', 'ERROR', etc.
  errorCode?: string; // Error code if resolution failed (e.g., 'FLAG_NOT_FOUND')
  errorMessage?: string; // Human-readable error message
}
```

> **Note:** The `expose` function is always available. When using auto-exposure (the default),
> calling `expose()` manually will log a warning in development mode and do nothing.

## Type Safety

The hooks validate that flag values match the type of your default value:

```tsx
// If the flag value is a string but you expect a number,
// the default value is returned instead
const limit = useFlag('pagination.limit', 10); // Returns 10 if flag value isn't a number

// Object structure is also validated
const config = useFlag('my-feature', { enabled: false, limit: 0 });
// Returns default if flag value doesn't have 'enabled' and 'limit' properties
```

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
const { value: showPromo, expose } = useFlagDetails('promo-banner.show', false, { expose: false });

const handleOpenModal = () => {
  if (showPromo) {
    expose(); // Only log when user actually opens the modal
    openPromoModal();
  }
};
```

### Specify flags for better performance

If your client is registered for many flags, but only need a few in the frontend, specify them to reduce the bundle size:

```tsx
<ConfidenceProvider evalContext={context} flags={['checkout-flow', 'promo-banner']}>
  {children}
</ConfidenceProvider>
```

### Use server hooks for server-only logic

If you're making a decision that only affects server rendering and doesn't need client interactivity, use the server hooks directly:

```tsx
// app/page.tsx
import { useFlag } from '@spotify-confidence/openfeature-server-provider-local/react-server';

export default async function Page() {
  const showNewLayout = await useFlag('page-layout.useNewDesign', false, { targetingKey: userId });

  // This decision is made entirely on the server
  return showNewLayout ? <NewLayout /> : <OldLayout />;
}
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
4. You're using dot notation to access the correct property (e.g., `my-flag.enabled` not just `my-flag`)
5. Check the output of `useFlagDetails` for `errorCode` and `errorMessage` to help diagnose issues.

### Exposure not being logged

- **Client hooks**: Make sure the component actually mounts. If using `{ expose: false }`, verify you're calling `expose()`.
- **Server hooks**: Exposure is logged immediately on evaluation - check server logs.
- Check that the provider is properly initialized and connected to Confidence.
