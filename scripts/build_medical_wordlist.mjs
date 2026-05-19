// Builds the bundled medical wordlist used by the in-app spellchecker.
//
// Source: CodeSante/medical-wordlist (en/wordlist.en.txt), license WTFPL.
// That upstream wordlist is derived from Wikidata SPARQL queries (Wikidata
// data itself is CC0). The bundled output file is therefore safe to ship
// from an MIT-licensed binary without copyleft obligations.
//
// What this script does:
//   1. Fetches the upstream multi-word wordlist.
//   2. Tokenizes each entry into individual words.
//   3. Keeps only ASCII-letter tokens of length >= 3 (drops the Arabic /
//      Chinese / Thai synonym entries and chromosomal-position fragments
//      like "11p15.4" that aren't useful for a per-word spellcheck pass).
//   4. Lowercases, dedupes, sorts.
//   5. Writes the result next to the spellchecker source so Vite can bundle
//      it as a `?url` asset.
//
// Re-run after upstream updates; commit both this script and the output.

import { writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const SOURCE_URL =
  'https://raw.githubusercontent.com/CodeSante/medical-wordlist/master/en/wordlist.en.txt';

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUTPUT_PATH = join(
  __dirname,
  '..',
  'src',
  'lib',
  'components',
  'rich_editor',
  'spellcheck',
  'medical_terms.txt',
);

const TOKEN_RE = /[a-zA-Z][a-zA-Z'-]{2,}/g;

async function main() {
  console.log(`Fetching ${SOURCE_URL}`);
  const res = await fetch(SOURCE_URL);
  if (!res.ok) throw new Error(`Download failed: HTTP ${res.status}`);
  const raw = await res.text();

  const tokens = new Set();
  for (const line of raw.split('\n')) {
    let match;
    TOKEN_RE.lastIndex = 0;
    while ((match = TOKEN_RE.exec(line)) !== null) {
      tokens.add(match[0].toLowerCase());
    }
  }

  const sorted = [...tokens].sort();
  await writeFile(OUTPUT_PATH, sorted.join('\n') + '\n', 'utf8');

  const bytes = Buffer.byteLength(sorted.join('\n'), 'utf8');
  console.log(`Wrote ${sorted.length} terms (${(bytes / 1024).toFixed(1)} KB) to`);
  console.log(`  ${OUTPUT_PATH}`);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
