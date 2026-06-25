/**
 * Shared Vite plugin that rewrites deep imports of `dictionary-en` Hunspell
 * asset files (index.aff / index.dic) to absolute node_modules paths.
 *
 * The `dictionary-en` package's "exports" field only exposes index.js, so deep
 * imports of the raw .aff/.dic files fail under Vite's strict subpath check.
 * This plugin lets the spellchecker wrapper import them as bundled assets —
 * no runtime network fetches, no PHI leakage.
 *
 * Extracted from the duplicated copies in vite.config.ts and vitest.config.ts
 * (AGENTS.md notes the duplication was intentional but must be kept in sync).
 */
import { fileURLToPath } from 'node:url';
import type { Plugin } from 'vite';

const affPath = fileURLToPath(
  new URL('../node_modules/dictionary-en/index.aff', import.meta.url),
);
const dicPath = fileURLToPath(
  new URL('../node_modules/dictionary-en/index.dic', import.meta.url),
);

export function dictionaryEnAssetResolver(): Plugin {
  return {
    name: 'dictionary-en-asset-resolver',
    enforce: 'pre',
    resolveId(source) {
      // Split off any Vite query (e.g. `?url`) so we can re-attach it.
      const [bare, query] = source.split('?');
      const suffix = query ? `?${query}` : '';
      if (bare === 'dictionary-en/index.aff') return affPath + suffix;
      if (bare === 'dictionary-en/index.dic') return dicPath + suffix;
      return null;
    },
  };
}
