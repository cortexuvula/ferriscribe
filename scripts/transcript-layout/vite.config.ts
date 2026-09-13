import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { dictionaryEnAssetResolver } from '../../vite-plugins/dictionary-en';
import { fileURLToPath } from 'node:url';

export default defineConfig({
  root: fileURLToPath(new URL('../../', import.meta.url)),
  plugins: [dictionaryEnAssetResolver(), svelte()],
  cacheDir: 'node_modules/.cache/transcript-layout',
  resolve: { alias: [{ find: /^@tauri-apps\/.*/, replacement: fileURLToPath(new URL('./tauri.ts', import.meta.url)) }] },
  define: { __APP_VERSION__: JSON.stringify('synthetic layout gate') },
  server: { host: '127.0.0.1', port: 14873, strictPort: true },
});
