#!/usr/bin/env node
// Build the ferriscribe-backup sidecar and stage it where Tauri expects
// external binaries: src-tauri/binaries/ferriscribe-backup-<target-triple>.
//
// Usage:
//   node scripts/build-sidecar.mjs [--target <triple>]
//
// Without --target, uses the host triple (correct for `npm run tauri dev`
// and local builds). CI release builds pass the matrix target explicitly
// because tauri-action builds with `--target`, and the suffix must match.
//
// cargo build is incremental — after the first build this is ~seconds.

import { execSync } from 'node:child_process';
import { copyFileSync, mkdirSync, existsSync, chmodSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

const targetArg = process.argv.indexOf('--target');
const target =
  targetArg !== -1 && process.argv[targetArg + 1]
    ? process.argv[targetArg + 1]
    : execSync('rustc -vV', { encoding: 'utf8' })
        .split('\n')
        .find((l) => l.startsWith('host:'))
        .split(':')[1]
        .trim();

console.log(`building ferriscribe-backup for ${target}…`);
execSync(`cargo build -p medical-backup --release --target ${target}`, {
  cwd: root,
  stdio: 'inherit',
});

const bin = process.platform === 'win32' ? 'ferriscribe-backup.exe' : 'ferriscribe-backup';
const src = join(root, 'target', target, 'release', bin);
const destDir = join(root, 'src-tauri', 'binaries');
const dest = join(destDir, `ferriscribe-backup-${target}`);

if (!existsSync(src)) {
  console.error(`sidecar binary not found after build: ${src}`);
  process.exit(1);
}
mkdirSync(destDir, { recursive: true });
copyFileSync(src, dest);
if (process.platform !== 'win32') chmodSync(dest, 0o755);
console.log(`staged sidecar: ${dest}`);
