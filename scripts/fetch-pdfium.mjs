#!/usr/bin/env node
// Downloads the prebuilt pdfium library for the CURRENT platform from
// bblanchon/pdfium-binaries (a trusted mirror of Google's official pdfium
// builds) and extracts it to src-tauri/resources/pdfium/, where Tauri bundles
// it as a resource (see tauri.conf.json `bundle.resources`).
//
// Run automatically via Tauri's `beforeDevCommand` / `beforeBuildCommand`
// (so it runs in both `tauri dev` and `tauri build`, including CI), or manually
// via `npm run fetch:pdfium`. Idempotent: skips the download when the pinned
// version is already on disk.
//
// The library is fetched at BUILD time only — there is NO network access at
// runtime (privacy-preserving). The fetched binaries are gitignored.

import { execSync } from 'node:child_process';
import {
  createWriteStream,
  existsSync,
  mkdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { dirname, join } from 'node:path';
import { Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, '..');
const OUT_DIR = join(ROOT, 'src-tauri', 'resources', 'pdfium');
const VERSION_FILE = join(OUT_DIR, '.version');

// Pinned bblanchon/pdfium-binaries release tag. Bump deliberately; pinning keeps
// builds reproducible. (Override via PDFIUM_BIN_VERSION env var for testing.)
const VERSION = process.env.PDFIUM_BIN_VERSION ?? 'chromium/7999';

// Map the current Node platform/arch to a pdfium-binaries archive + the lib
// member inside it + the normalized output filename. The output filenames are
// the platform-default names that pdfium-render's
// `pdfium_platform_library_name_at_path(dir)` looks for.
function target() {
  const { platform, arch } = process;
  if (platform === 'darwin' && arch === 'arm64')
    return { asset: 'pdfium-mac-arm64.tgz', member: 'lib/libpdfium.dylib', out: 'libpdfium.dylib' };
  if (platform === 'darwin' && arch === 'x64')
    return { asset: 'pdfium-mac-x64.tgz', member: 'lib/libpdfium.dylib', out: 'libpdfium.dylib' };
  if (platform === 'linux' && arch === 'x64')
    return { asset: 'pdfium-linux-x64.tgz', member: 'lib/libpdfium.so', out: 'libpdfium.so' };
  if (platform === 'win32' && arch === 'x64')
    return { asset: 'pdfium-win-x64.tgz', member: 'bin/pdfium.dll', out: 'pdfium.dll' };
  throw new Error(`fetch-pdfium: unsupported platform/arch: ${platform}/${arch}`);
}

async function main() {
  const { asset, member, out } = target();
  const outPath = join(OUT_DIR, out);

  // Idempotency: skip when the pinned version is already fetched.
  if (
    existsSync(outPath) &&
    existsSync(VERSION_FILE) &&
    readFileSync(VERSION_FILE, 'utf8').trim() === VERSION
  ) {
    console.log(`fetch-pdfium: ${out} already present (${VERSION}), skipping.`);
    return;
  }

  mkdirSync(OUT_DIR, { recursive: true });
  const url = `https://github.com/bblanchon/pdfium-binaries/releases/download/${VERSION}/${asset}`;
  console.log(`fetch-pdfium: downloading ${url}`);

  // Stream the .tgz to a temp file in OUT_DIR.
  const tmpArchive = join(OUT_DIR, 'archive.tgz');
  const res = await fetch(url, { redirect: 'follow' });
  if (!res.ok || !res.body) {
    throw new Error(`fetch-pdfium: HTTP ${res.status} for ${url}`);
  }
  await pipeline(Readable.fromWeb(res.body), createWriteStream(tmpArchive));

  // Extract just the library member using the system `tar` (bsdtar ships with
  // macOS, Linux, and Windows 10 1803+; GitHub runners have it everywhere).
  console.log(`fetch-pdfium: extracting ${member}`);
  try {
    execSync(`tar -xzf "${tmpArchive}" -C "${OUT_DIR}" "${member}"`, { stdio: 'inherit' });
  } catch (e) {
    throw new Error(`fetch-pdfium: tar extraction failed: ${e instanceof Error ? e.message : e}`);
  }

  // The member extracts nested (lib/... or bin/...); move it to the OUT_DIR root
  // with the normalized platform name, then remove the now-empty subdirs.
  const extracted = join(OUT_DIR, member);
  rmSync(outPath, { force: true });
  renameSync(extracted, outPath);
  rmSync(join(OUT_DIR, dirname(member)), { recursive: true, force: true });
  rmSync(tmpArchive, { force: true });

  // On macOS, sign the dylib so it passes app notarization. Tauri's bundler
  // signs the main executable and the .app but NOT resource dylibs under
  // Contents/Resources/ — Apple's notarization rejects an unsigned/ad-hoc
  // dylib with "Archive contains critical validation errors". We re-sign it
  // here with the Developer ID + Hardened Runtime when the identity is present
  // (CI exposes APPLE_SIGNING_IDENTITY during beforeBuildCommand). Local dev
  // has no identity → the dylib stays unsigned, which is fine (no notarization
  // outside CI). Done BEFORE stamping .version so a codesign failure forces a
  // re-fetch on the next run rather than persisting an unstamped "success".
  if (process.platform === 'darwin') {
    // Strip extended attributes (prevents "resource fork / Finder information
    // / similar detritus not allowed" codesign rejection).
    try {
      execSync(`xattr -cr "${outPath}"`, { stdio: 'ignore' });
    } catch {
      /* xattr may be unavailable; non-fatal */
    }
    if (process.env.APPLE_SIGNING_IDENTITY) {
      console.log(`fetch-pdfium: codesigning ${out} (Hardened Runtime, Developer ID)`);
      try {
        execSync(
          `codesign --force --options runtime --sign "${process.env.APPLE_SIGNING_IDENTITY}" "${outPath}"`,
          { stdio: 'inherit' },
        );
      } catch (e) {
        throw new Error(`fetch-pdfium: codesign failed: ${e instanceof Error ? e.message : e}`);
      }
    } else {
      console.log(`fetch-pdfium: APPLE_SIGNING_IDENTITY not set — leaving ${out} unsigned (fine for dev)`);
    }
  }

  writeFileSync(VERSION_FILE, VERSION);
  console.log(`fetch-pdfium: wrote ${outPath} (${VERSION})`);
}

main().catch((e) => {
  console.error(e instanceof Error ? e.message : e);
  process.exit(1);
});
