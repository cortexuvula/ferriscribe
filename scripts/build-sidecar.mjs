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

import { execFileSync } from 'node:child_process';
// --debug builds the default (dev) profile and stages from there. CI's
// lint/test jobs use it: tauri-build validates externalBin EXISTENCE at
// compile time, those jobs never bundle, and the dev profile reuses the
// artifacts they already compiled for the workspace — near-zero cost.
const debug = process.argv.includes('--debug');
import { copyFileSync, mkdirSync, existsSync, chmodSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

const targetArg = process.argv.indexOf('--target');
const target =
  targetArg !== -1 && process.argv[targetArg + 1]
    ? process.argv[targetArg + 1]
    : (() => {
        const host = execFileSync('rustc', ['-vV'], { encoding: 'utf8' })
          .split('\n')
          .find((l) => l.startsWith('host:'))
          ?.split(':')[1]
          ?.trim();
        if (!host) {
          console.error('could not determine host target triple from `rustc -vV`');
          process.exit(1);
        }
        return host;
      })();

// The triple is interpolated into paths and (below) handed to cargo as a
// single argv element — but validate its shape anyway so a malformed
// --target value fails loudly here instead of downstream.
if (!/^[A-Za-z0-9_-]+$/.test(target)) {
  console.error(`invalid target triple: ${target}`);
  process.exit(1);
}

console.log(`building ferriscribe-backup for ${target}…`);
// execFileSync, never a shell string: the triple is untrusted argv input.
execFileSync(
  'cargo',
  ['build', '-p', 'medical-backup', ...(debug ? [] : ['--release']), '--target', target],
  { cwd: root, stdio: 'inherit' }
);

// Derive the binary name AND staged extension from the TARGET triple, not
// the host platform — cross-compiling a Windows target on macOS/Linux must
// stage ferriscribe-backup-<triple>.exe (Tauri resolves externalBin with
// the extension for Windows bundle targets), and vice versa.
const targetIsWindows = /-windows-(msvc|gnu)$/.test(target);
const bin = targetIsWindows ? 'ferriscribe-backup.exe' : 'ferriscribe-backup';
const src = join(root, 'target', target, debug ? 'debug' : 'release', bin);
const destDir = join(root, 'src-tauri', 'binaries');
const ext = targetIsWindows ? '.exe' : '';
const dest = join(destDir, `ferriscribe-backup-${target}${ext}`);

if (!existsSync(src)) {
  console.error(`sidecar binary not found after build: ${src}`);
  process.exit(1);
}
mkdirSync(destDir, { recursive: true });
copyFileSync(src, dest);
if (!targetIsWindows) chmodSync(dest, 0o755);
console.log(`staged sidecar: ${dest}`);
