import { readFile, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { defineConfig } from 'tsup';

const OUT_DIR = 'dist';

/**
 * Bundles that must keep the `'use client'` directive. esbuild drops
 * module-level directives when bundling and the rollup tree-shaking pass
 * drops them again, so they are re-attached after the build completes.
 */
const CLIENT_BUNDLES = ['provider.js', 'provider.cjs'];

async function restoreUseClientDirective(): Promise<void> {
  await Promise.all(
    CLIENT_BUNDLES.map(async (file) => {
      const filePath = path.join(OUT_DIR, file);
      const code = await readFile(filePath, 'utf8');
      if (/^\s*(['"])use client\1/.test(code)) return;
      await writeFile(filePath, `'use client';\n${code}`);
    }),
  );
}

export default defineConfig({
  entry: [
    'src/index.ts',
    'src/server.ts',
    'src/provider.tsx',
    'src/pages.ts',
    'src/middleware.ts',
    'src/api.ts',
  ],
  format: ['esm', 'cjs'],
  dts: true,
  splitting: false,
  sourcemap: true,
  clean: true,
  treeshake: true,
  minify: true,
  target: 'es2022',
  outDir: OUT_DIR,
  external: [
    'next',
    'react',
    'react-dom',
    '@darshjdb/client',
    '@darshjdb/react',
  ],
  esbuildOptions(options) {
    options.jsx = 'automatic';
  },
  onSuccess: restoreUseClientDirective,
});
