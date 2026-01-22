import { defineConfig } from 'tsdown';

export default defineConfig({
  entry: './src/index.ts',
  platform: 'browser',
  minify: 'dce-only',
  dts: { oxc: true },
  define: {
    __ASSERT__: 'false',
    __TEST__: 'false',
  },
  external: ['react', 'react/jsx-runtime'],
});
