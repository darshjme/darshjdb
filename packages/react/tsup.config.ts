import { defineConfig } from 'tsup';

export default defineConfig([
  {
    entry: ['src/index.ts'],
    format: ['esm', 'cjs'],
    dts: false,
    splitting: false,
    sourcemap: true,
    clean: true,
    treeshake: true,
    minify: true,
    target: 'es2022',
    outDir: 'dist',
    external: ['react', '@darshan/client'],
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
    external: ['react', '@darshan/client'],
    jsx: 'automatic',
  },
]);
