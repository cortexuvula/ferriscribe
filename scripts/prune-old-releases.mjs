#!/usr/bin/env node
/**
 * Prunes old GitHub releases, keeping only the N newest (by semver).
 *
 * Deletes both the release (binary assets + release page) AND its git tag
 * for every release beyond the keep-count. Run automatically after each
 * release build by the Release workflow, or manually for one-time cleanup.
 *
 * Usage:
 *   node scripts/prune-old-releases.mjs           # live run, keeps 5
 *   node scripts/prune-old-releases.mjs --dry-run # preview what would be deleted
 *   KEEP=3 node scripts/prune-old-releases.mjs     # keep 3 instead of 5
 *
 * Requires GH_TOKEN (or GITHUB_TOKEN) env var with repo:delete scope.
 * Only deletes releases whose tag matches /^v\d/ (FerriScribe releases);
 * leaves non-version tags (whisper-server-*) untouched.
 */
import { execSync } from 'node:child_process';

const KEEP = parseInt(process.env.KEEP ?? '5', 10);
const DRY_RUN = process.argv.includes('--dry-run');

/** Run a gh CLI command and return trimmed stdout. */
function gh(...args) {
  return execSync(['gh', ...args].join(' '), { encoding: 'utf8' }).trim();
}

/** Run a gh command, return true on success / false on failure. */
function ghOk(...args) {
  try {
    execSync(['gh', ...args].join(' '), { encoding: 'utf8', stdio: 'pipe' });
    return true;
  } catch {
    return false;
  }
}

// 1. List all releases as JSON, sorted newest-first by semver tag.
const raw = gh('release', 'list', '--limit', '200', '--json', 'tagName');
const tags = JSON.parse(raw)
  .map((r) => r.tagName)
  // Only FerriScribe version tags (vX.Y.Z or vX.Y.Z-beta.N).
  .filter((t) => /^v\d+\.\d+\.\d+/.test(t))
  // Semver-aware sort: split into numeric parts, compare descending.
  .sort((a, b) => {
    const pa = a.replace(/^v/, '').split(/[.-]/).map((x) => parseInt(x, 10) || 0);
    const pb = b.replace(/^v/, '').split(/[.-]/).map((x) => parseInt(x, 10) || 0);
    for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
      const d = (pb[i] ?? 0) - (pa[i] ?? 0);
      if (d !== 0) return d;
    }
    return 0;
  });

const keep = tags.slice(0, KEEP);
const delete_ = tags.slice(KEEP);

console.log(`Release prune: ${tags.length} versioned releases found.`);
console.log(`Keeping ${keep.length} newest: ${keep.join(', ')}`);
console.log(`${delete_.length} to delete${DRY_RUN ? ' (DRY RUN)' : ''}:`);

if (DRY_RUN) {
  for (const t of delete_) console.log(`  would delete: ${t}`);
  console.log(`\nDry run complete. ${delete_.length} releases would be deleted.`);
  process.exit(0);
}

let deleted = 0;
let failed = 0;
for (const tag of delete_) {
  // --cleanup-tag deletes the git ref alongside the release.
  if (ghOk('release', 'delete', tag, '--yes', '--cleanup-tag')) {
    deleted++;
    console.log(`  deleted: ${tag}`);
  } else {
    // Retry: release may already be gone but tag lingers.
    if (ghOk('api', '-X', 'DELETE', `repos/:owner/:repo/git/refs/tags/${tag}`)) {
      deleted++;
      console.log(`  deleted (tag-only): ${tag}`);
    } else {
      failed++;
      console.log(`  FAILED: ${tag}`);
    }
  }
}

console.log(`\nDone: ${deleted} deleted, ${failed} failed.`);
