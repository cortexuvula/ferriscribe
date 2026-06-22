import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';
import { readFileSync } from 'node:fs';
import type { Plugin } from 'vite';

// Read the app version from package.json so the About pane shows it without
// a separate Tauri invoke.
const pkg = JSON.parse(readFileSync(fileURLToPath(new URL('./package.json', import.meta.url)), 'utf-8'));

const host = process.env.TAURI_DEV_HOST;

// Resolve dictionary-en asset paths directly. The package's "exports" field
// only exposes index.js, so deep imports of the raw Hunspell .aff/.dic files
// fail under Vite's strict subpath check. This tiny plugin rewrites those
// specifiers (with or without the `?url` query) to absolute node_modules
// paths, letting the spellchecker wrapper import them as bundled assets —
// no runtime network fetches, no PHI leakage.
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
      // Split off any Vite query (e.g. `?url`) so we can re-attach it.
      const [bare, query] = source.split('?');
      const suffix = query ? `?${query}` : '';
      if (bare === 'dictionary-en/index.aff') return dictionaryEnAffPath + suffix;
      if (bare === 'dictionary-en/index.dic') return dictionaryEnDicPath + suffix;
      return null;
    },
  };
}

// https://vitejs.dev/config/
export default defineConfig(async () => ({
  plugins: [dictionaryEnAssetResolver(), svelte()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: 'ws',
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell vite to ignore watching `src-tauri`
      ignored: ['**/src-tauri/**', '**/target/**'],
    },
  },

  // Expose the app version (from package.json) to the frontend for the About pane.
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
}));
