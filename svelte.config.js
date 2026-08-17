import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

const config = {
  preprocess: vitePreprocess(),
  // The app is runes-only. Setting it here (rather than per-config) forces the
  // same compile mode in `vite.config.ts` and `vitest.config.ts`, so a
  // component cannot test green under forced runes yet compile legacy in the
  // app build.
  compilerOptions: {
    runes: true,
  },
};

export default config;
