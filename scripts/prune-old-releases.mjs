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
 * Only deletes releases whose tag matches a clean semver pattern
 * (vX.Y.Z or vX.Y.Z-suffix); leaves non-version tags untouched.
 */
import { execSync } from 'node:child_process';

// Only match clean semver tags: vX.Y.Z or vX.Y.Z-suffix (no shell metacharacters).
// Fully anchored to reject tags like "v1.0.0; rm -rf /".
const VERSION_RE = /^v\d+\.\d+\.\d+(-[a-zA-Z0-9.]+)?$/;
// Reject tags containing shell metacharacters (defense against injection).
const SHELL_SAFE = /^[a-zA-Z0-9._-]+$/;

// Validate KEEP: must be a positive integer. A bad value (e.g. "abc") would
// produce NaN, and slice(0, NaN) === [] → every release deleted.
let KEEP;
const KEEP_RAW = process.env.KEEP ?? '5';
if (!/^\d+$/.test(KEEP_RAW) || Number(KEEP_RAW) < 1) {
  console.error(
    `Invalid KEEP value: "${process.env.KEEP}". Must be a positive integer. Defaulting to 5.`,
  );
  KEEP = 5;
} else {
  KEEP = Number(KEEP_RAW);
}

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

/**
 * Full semver precedence comparison (semver.org §11) for the tag shapes
 * this repo uses: vX.Y.Z and vX.Y.Z-prerelease (dot-separated identifiers).
 *
 * The previous parseInt-based comparator coerced prerelease identifiers to
 * 0, which ranked v1.0.0-beta.1 ABOVE v1.0.0 — exactly backwards — and so
 * pruned the stable release while keeping its beta.
 *
 * Returns a negative number when a has LOWER precedence than b.
 */
function compareSemver(a, b) {
  const pa = a.replace(/^v/, '');
  const pb = b.replace(/^v/, '');
  const [coreA, preA = ''] = pa.split('-', 2);
  const [coreB, preB = ''] = pb.split('-', 2);
  const numsA = coreA.split('.').map(Number);
  const numsB = coreB.split('.').map(Number);
  for (let i = 0; i < 3; i++) {
    const d = (numsA[i] ?? 0) - (numsB[i] ?? 0);
    if (d !== 0) return d;
  }
  // Equal cores: a version WITHOUT a prerelease has HIGHER precedence.
  const hasPreA = preA.length > 0;
  const hasPreB = preB.length > 0;
  if (!hasPreA && !hasPreB) return 0;
  if (!hasPreA) return 1;
  if (!hasPreB) return -1;
  // Compare prerelease identifiers: numeric < alphanumeric, numerics
  // numerically, alphanumerics lexically (ASCII); fewer identifiers is
  // lower when all preceding are equal.
  const idsA = preA.split('.');
  const idsB = preB.split('.');
  for (let i = 0; i < Math.max(idsA.length, idsB.length); i++) {
    const x = idsA[i];
    const y = idsB[i];
    if (x === undefined) return -1;
    if (y === undefined) return 1;
    const xNum = /^\d+$/.test(x);
    const yNum = /^\d+$/.test(y);
    if (xNum && yNum) {
      const d = Number(x) - Number(y);
      if (d !== 0) return d;
    } else if (xNum) {
      return -1; // numeric identifiers have lower precedence
    } else if (yNum) {
      return 1;
    } else {
      const d = x < y ? -1 : x > y ? 1 : 0;
      if (d !== 0) return d;
    }
  }
  return 0;
}

// 1. List all releases as JSON, sorted newest-first by semver tag.
const raw = gh('release', 'list', '--limit', '200', '--json', 'tagName');
const tags = JSON.parse(raw)
  .map((r) => r.tagName)
  // Only FerriScribe version tags (vX.Y.Z or vX.Y.Z-beta.N), fully anchored.
  .filter((t) => VERSION_RE.test(t))
  // Semver-aware sort, highest precedence first.
  .sort((a, b) => compareSemver(a, b) * -1);

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
let tagOnly = 0;
let failed = 0;
for (const tag of delete_) {
  // Defense in depth: skip any tag that slipped past the regex but contains
  // shell metacharacters, since gh args are joined into a single shell string.
  if (!SHELL_SAFE.test(tag)) {
    console.warn(`Skipping tag with unsafe characters: ${tag}`);
    continue;
  }
  // --cleanup-tag deletes the git ref alongside the release.
  if (ghOk('release', 'delete', tag, '--yes', '--cleanup-tag')) {
    deleted++;
    console.log(`  deleted: ${tag}`);
  } else {
    // Retry: release may already be gone but tag lingers. This is a
    // PARTIAL outcome — the release page/assets may still exist (e.g. a
    // transient release-delete failure); counted separately so it is not
    // reported as a clean delete. It is retried on the next prune run
    // because enumeration is by release, not by tag.
    if (ghOk('api', '-X', 'DELETE', `repos/:owner/:repo/git/refs/tags/${tag}`)) {
      tagOnly++;
      console.log(`  deleted tag only (release may remain): ${tag}`);
    } else {
      failed++;
      console.log(`  FAILED: ${tag}`);
    }
  }
}

console.log(`\nDone: ${deleted} deleted, ${tagOnly} tag-only (release may remain), ${failed} failed.`);
