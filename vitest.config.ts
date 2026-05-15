import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';

export default defineConfig({
  plugins: [svelte({ compilerOptions: { runes: true } })],
  resolve: {
    // Allow bare imports (no extension) to resolve .svelte.ts files so that
    // test and consumer imports like `from './recordSidebar'` still work
    // after stores are renamed from .ts → .svelte.ts.
    extensions: ['.svelte.ts', '.mjs', '.js', '.ts', '.jsx', '.tsx', '.json'],
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
    setupFiles: ['src/test-setup.localStorage.ts'],
  },
});
