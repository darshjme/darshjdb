import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    projects: [
      'packages/client-core/vitest.config.ts',
      'packages/react/vitest.config.ts',
    ],
  },
});
