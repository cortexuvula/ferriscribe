import { defineConfig } from 'vite';
import { dictionaryEnAssetResolver } from '../../vite-plugins/dictionary-en';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../..', import.meta.url));
export default defineConfig({ root, plugins: [dictionaryEnAssetResolver(), svelte()], resolve: { alias: [{ find: /^@tauri-apps\/.*/, replacement: fileURLToPath(new URL('./browser-tauri.ts', import.meta.url)) }] }, define: { __APP_VERSION__: JSON.stringify('SYNTHETIC FIXTURE') }, server: {host: '127.0.0.1', port: 14739, strictPort: true}, cacheDir: 'docs/settings-declutter/browser-cache' });
