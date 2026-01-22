import { defineConfig } from 'vitest/config';

export default defineConfig({
  define: {
    __TEST__: 'true',
    __ASSERT__: 'true',
  },
  test: {
    environment: 'jsdom',
    globals: false,
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
  },
});
