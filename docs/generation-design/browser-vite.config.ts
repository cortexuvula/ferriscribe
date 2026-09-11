import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../..', import.meta.url));
export default defineConfig({root, plugins:[svelte()], resolve:{alias:[{find:/^@tauri-apps\/.*/,replacement:fileURLToPath(new URL('./browser-tauri.ts',import.meta.url))}]},cacheDir:'node_modules/.cache/generation-browser',server:{host:'127.0.0.1',port:14749,strictPort:true},define:{__APP_VERSION__:JSON.stringify('SYNTHETIC HARNESS')}});
