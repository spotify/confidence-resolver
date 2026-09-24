import { fileURLToPath } from 'node:url';
import { rolldown } from 'rolldown';
import { expect, it } from 'vitest';

it('bundles the local provider without Node runtime dependencies', async () => {
  const bundle = await rolldown({
    input: fileURLToPath(new URL('./ConfidenceServerProviderLocal.ts', import.meta.url)),
    platform: 'browser',
    external: ['@bufbuild/protobuf/wire', 'debug'],
    onwarn(warning, defaultHandler) {
      if (warning.code === 'UNRESOLVED_IMPORT') {
        throw new Error(warning.message);
      }
      defaultHandler(warning);
    },
  });
  try {
    const { output } = await bundle.generate({ format: 'esm' });
    for (const chunk of output) {
      if (chunk.type === 'chunk') {
        expect(chunk.imports.every(id => id === '@bufbuild/protobuf/wire' || id === 'debug')).toBe(true);
      }
    }
  } finally {
    await bundle.close();
  }
});
