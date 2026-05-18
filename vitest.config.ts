import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';
import type { Plugin } from 'vite';

// Mirror the dictionary-en asset resolver from vite.config.ts so spellcheck
// tests can vi.mock the same import specifiers without Vite's strict exports
// check rejecting the deep subpath.
const dictionaryEnAffPath = fileURLToPath(
  new URL('./node_modules/dictionary-en/index.aff', import.meta.url),
);
const dictionaryEnDicPath = fileURLToPath(
  new URL('./node_modules/dictionary-en/index.dic', import.meta.url),
);

function dictionaryEnAssetResolver(): Plugin {
  return {
    name: 'dictionary-en-asset-resolver',
    enforce: 'pre',
    resolveId(source) {
      const [bare, query] = source.split('?');
      const suffix = query ? `?${query}` : '';
      if (bare === 'dictionary-en/index.aff') return dictionaryEnAffPath + suffix;
      if (bare === 'dictionary-en/index.dic') return dictionaryEnDicPath + suffix;
      return null;
    },
  };
}

export default defineConfig({
  plugins: [dictionaryEnAssetResolver(), svelte({ compilerOptions: { runes: true } })],
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
