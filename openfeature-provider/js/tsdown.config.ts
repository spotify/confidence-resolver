import { defineConfig } from 'tsdown';

const base = defineConfig({
  minify: 'dce-only',
  dts: {
    oxc: true,
  },
  define: {
    __ASSERT__: 'false',
    __TEST__: 'false',
  },
  external: ['@bufbuild/protobuf/wire'],
});

const reactBase = defineConfig({
  minify: 'dce-only',
  dts: {
    oxc: true,
  },
  define: {
    __ASSERT__: 'false',
    __TEST__: 'false',
  },
  external: ['@bufbuild/protobuf/wire', 'react', '@openfeature/server-sdk', '@openfeature/core'],
});

export default defineConfig([
  // Single universal entry: state modules are fetched at runtime, so there is
  // no wasm binary to inline, copy or locate per platform.
  {
    entry: './src/index.ts',
    platform: 'neutral',
    ...base,
  },
  // React server component
  {
    entry: './src/react/server.tsx',
    platform: 'neutral',
    ...reactBase,
    external: [...(reactBase.external || []), './client'],
  },
  // React client components
  {
    entry: './src/react/client.tsx',
    platform: 'neutral',
    ...reactBase,
  },
  // Pages Router: server-side helpers (withConfidence, resolveConfidence)
  {
    entry: { 'pages-router/server': './src/pages-router/server.ts' },
    platform: 'neutral',
    ...reactBase,
    external: [...(reactBase.external || []), 'next', /^node:/],
  },
  // Pages Router: client wrapper used in _app.tsx. Externalize the
  // package self-reference to react-client so we don't bundle a second copy
  // of ConfidenceContext (which would silently break useFlag/useFlagDetails).
  {
    entry: { 'pages-router/client': './src/pages-router/client.tsx' },
    platform: 'neutral',
    ...reactBase,
    external: [
      ...(reactBase.external || []),
      'next',
      '@spotify-confidence/openfeature-server-provider-local/react-client',
    ],
  },
  // Pages Router: applyHandler factory for the API route
  {
    entry: { 'pages-router/api': './src/pages-router/api.ts' },
    platform: 'neutral',
    ...reactBase,
    external: [...(reactBase.external || []), 'next', /^node:/],
  },
]);
