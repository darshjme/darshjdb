import { defineConfig } from 'tsup';

export default defineConfig([
  {
    entry: ['src/index.ts'],
    format: ['esm', 'cjs'],
    dts: false,
    splitting: false,
    sourcemap: true,
    clean: true,
    // The rollup treeshake pass strips module-level directives; esbuild keeps
    // the entry's `'use client'`, which the Next.js App Router requires.
    treeshake: false,
    minify: true,
    target: 'es2022',
    outDir: 'dist',
    external: ['react', '@darshjdb/client'],
    jsx: 'automatic',
  },
  {
    entry: ['src/index.ts'],
    format: ['esm'],
    dts: { only: true },
    splitting: false,
    clean: false,
    target: 'es2022',
    outDir: 'dist',
    external: ['react', '@darshjdb/client'],
    jsx: 'automatic',
  },
]);
