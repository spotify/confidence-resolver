import { defineConfig } from 'vitest/config';
import { parse } from 'dotenv';
import { existsSync, readFileSync } from 'fs';

function loadEnv(path: string): Record<string, string> {
  if (!existsSync(path)) return {};
  return parse(readFileSync(path, 'utf-8'));
}

export default defineConfig({
  define: {
    __TEST__: 'true',
    __ASSERT__: 'true',
  },
  test: {
    environment: 'node',
    globals: false,
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
    silent: false,
    watch: false,
    env: loadEnv('.env.test'),
  },
});
