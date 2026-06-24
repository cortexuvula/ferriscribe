#!/usr/bin/env node
// Asserts the FerriScribe version is identical across the three files that
// must stay in sync (AGENTS.md "Versioning" section):
//   - src-tauri/Cargo.toml
//   - package.json
//   - src-tauri/tauri.conf.json
//
// Exits non-zero on mismatch so CI fails before a wrong-versioned installer
// can be released (a mismatch breaks the auto-updater, which consumes
// latest.json keyed by version). Run locally via `node scripts/check-version-sync.mjs`.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

function readText(rel) {
  return readFileSync(join(root, rel), 'utf8');
}

function extract(label, text, pattern, parseErr) {
  const m = text.match(pattern);
  if (!m) {
    console.error(`✗ ${parseErr}`);
    process.exit(1);
  }
  return { label, version: m[1] };
}

const cargo = extract(
  'src-tauri/Cargo.toml',
  readText('src-tauri/Cargo.toml'),
  /^version\s*=\s*"([^"]+)"/m,
  'could not find `version = "..."` in src-tauri/Cargo.toml',
);
const pkg = extract(
  'package.json',
  readText('package.json'),
  /"version"\s*:\s*"([^"]+)"/,
  'could not find `"version": "..."` in package.json',
);
const tauri = extract(
  'src-tauri/tauri.conf.json',
  readText('src-tauri/tauri.conf.json'),
  /"version"\s*:\s*"([^"]+)"/,
  'could not find `"version": "..."` in src-tauri/tauri.conf.json',
);

const sources = [cargo, pkg, tauri];
const versions = new Set(sources.map((s) => s.version));

if (versions.size === 1) {
  console.log(`✓ version in sync across all 3 files: ${cargo.version}`);
  process.exit(0);
}

console.error('✗ version mismatch across files:');
for (const s of sources) {
  console.error(`    ${s.label.padEnd(28)} → ${s.version}`);
}
console.error('');
console.error('Update all three together (see AGENTS.md "Versioning").');
process.exit(1);
