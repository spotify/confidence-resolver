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

export default defineConfig([
  // Default: inlined WASM as data URL (works everywhere)
  {
    entry: './src/index.inlined.ts',
    platform: 'neutral',
    inputOptions: {
      moduleTypes: {
        '.wasm': 'dataurl',
      },
    },
    ...base,
  },
  // ./node: uses fs.readFile (traditional Node.js)
  {
    entry: './src/index.node.ts',
    platform: 'node',
    copy: ['../../../../wasm/confidence_resolver.wasm'],
    ...base,
  },
  // ./fetch: uses fetch + URL (Deno, Bun, browsers with good bundlers)
  {
    entry: './src/index.fetch.ts',
    platform: 'neutral',
    copy: ['../../../../wasm/confidence_resolver.wasm'],
    ...base,
  },
]);
