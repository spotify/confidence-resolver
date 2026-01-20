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
  // inputOptions: {
  //   moduleTypes: {
  //     '.wasm':'asset'
  //   }
  // },
});

export default defineConfig([
  {
    entry: './src/index.node.ts',
    platform: 'node',
    copy: ['../../wasm/confidence_resolver.wasm'],
    ...base,
  },
  {
    entry: './src/index.browser.ts',
    platform: 'browser',
    ...base,
  },
  {
    // React entry - lightweight, no WASM or protobuf dependency
    entry: './src/index.react.ts',
    platform: 'browser',
    minify: 'dce-only',
    dts: {
      oxc: true,
    },
    define: {
      __ASSERT__: 'false',
      __TEST__: 'false',
    },
    external: ['react', 'react/jsx-runtime'],
  },
]);
