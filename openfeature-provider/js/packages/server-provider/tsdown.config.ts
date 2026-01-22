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
  {
    entry: './src/index.node.ts',
    platform: 'node',
    copy: ['../../../../wasm/confidence_resolver.wasm'],
    ...base,
  },
  {
    entry: './src/index.browser.ts',
    platform: 'browser',
    ...base,
  },
]);
