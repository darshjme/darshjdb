import { defineConfig } from 'tsup';

export default defineConfig({
  entry: ['src/index.ts'],
  format: ['esm', 'cjs'],
  dts: true,
  splitting: false,
  sourcemap: true,
  clean: true,
  treeshake: true,
  minify: true,
  target: 'es2022',
  outDir: 'dist',
  globalName: 'DarshJDB',
  external: [],
  esbuildOptions(options) {
    options.mangleProps = /^_private/;
  },
});
