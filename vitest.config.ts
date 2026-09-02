import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { dictionaryEnAssetResolver } from './vite-plugins/dictionary-en';

export default defineConfig({
  // Runes mode comes from svelte.config.js (shared with vite.config.ts).
  plugins: [dictionaryEnAssetResolver(), svelte()],
  resolve: {
    // Allow bare imports (no extension) to resolve .svelte.ts files so that
    // test and consumer imports like `from './recordSidebar'` still work
    // after stores are renamed from .ts → .svelte.ts.
    extensions: ['.svelte.ts', '.mjs', '.js', '.ts', '.jsx', '.tsx', '.json'],
    // Svelte 5 ships two entry points: `browser` (has `mount()`) and
    // `default`/`worker` (server, no `mount()`). Under Vitest the `browser`
    // export condition is not active by default, so `import ... from 'svelte'`
    // resolves to the server build and component render tests blow up with
    // "lifecycle_function_unavailable / mount is not available on the server".
    // Activating the `browser` condition makes bare `svelte` imports resolve to
    // the client build everywhere. This is safe for the node-environment store
    // tests too: they import `.svelte.ts` modules (runes stores), not
    // `svelte`'s server DOM APIs.
    conditions: ['browser'],
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
    setupFiles: ['src/test-setup.localStorage.ts'],
    // The default environment is "node" (no DOM) for the store/util tests.
    // Component render tests opt into jsdom via a `// @vitest-environment jsdom`
    // file pragma. For those, Svelte must compile *client* components (its
    // `mount()` / lifecycle hooks only exist on the client); vite-plugin-svelte
    // picks server vs client from the environment's `consumer`, which vitest
    // leaves unset (defaults to server) unless we force it here.
    environmentOptions: {
      jsdom: {
        consumer: 'client',
      },
    },
    coverage: {
      provider: 'v8',
      reporter: ['text', 'html', 'lcov'],
      include: ['src/**/*.{ts,svelte}'],
      exclude: ['src/**/*.test.ts', 'src/test-setup.*'],
      // Floors, not targets — calibrated to just under the measured
      // baseline (2026-08-25) so coverage drift fails CI instead of
      // silently eroding. Ratchet up as the known gaps close.
      thresholds: [
        // Workspace-wide floor (well-tested stores/utils mask these).
        {
          lines: 25,
          statements: 25,
          functions: 30,
          branches: 20,
        },
        // Per-area floors (2026-09-02, measured baseline): without them the
        // big component/page trees (Models, BackupWizard, EditorTab…) hide
        // under the global average. Ratchet up as component tests land.
        {
          glob: 'src/lib/components/**',
          lines: 21,
          statements: 24,
          functions: 24,
          branches: 12,
        },
        {
          glob: 'src/lib/pages/**',
          lines: 9,
          statements: 12,
          functions: 16,
          branches: 5,
        },
      ],
    },
  },
});
