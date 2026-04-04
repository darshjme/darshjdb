import { defineConfig } from 'tsup';

export default defineConfig({
  entry: ['src/public-api.ts'],
  format: ['esm', 'cjs'],
  dts: true,
  splitting: false,
  sourcemap: true,
  clean: true,
  treeshake: true,
  minify: true,
  target: 'es2022',
  outDir: 'dist',
  external: [
    '@angular/core',
    '@angular/common',
    '@angular/router',
    '@angular/common/http',
    '@angular/platform-browser',
    'rxjs',
    'rxjs/operators',
  ],
});
