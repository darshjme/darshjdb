import { defineConfig } from 'tsup';

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
  outDir: 'dist',
  external: [
    'next',
    'react',
    'react-dom',
    '@darshan/client',
    '@darshan/react',
  ],
  esbuildOptions(options) {
    options.jsx = 'automatic';
  },
});
