#!/usr/bin/env node
/**
 * Prunes old GitHub releases with SEPARATE stable and prerelease retention
 * (U2 fix), so prereleases can never displace the stable update channel.
 *
 * The app's updater resolves through GitHub's /releases/latest endpoint,
 * which excludes prereleases — if pruning deleted every stable release
 * (the old behavior: one mixed pool, KEEP newest overall), update clients
 * would have no usable release. Retention is now two pools:
 *
 *   - stable (vX.Y.Z):      keep the STABLE_KEEP newest
 *   - prerelease (vX.Y.Z-*): keep the PRE_KEEP newest
 *
 * Drafts are never deleted by this script (a draft is mid-flight in the
 * Release workflow — its lifecycle belongs to that workflow's cleanup job).
 *
 * Deletes both the release (binary assets + release page) AND its git tag
 * for every release beyond the keep-count. Run automatically after each
 * release build by the Release workflow, or manually for one-time cleanup.
 *
 * Usage:
 *   node scripts/prune-old-releases.mjs           # live run
 *   node scripts/prune-old-releases.mjs --dry-run # preview what would be deleted
 *   STABLE_KEEP=3 PRE_KEEP=5 node scripts/prune-old-releases.mjs
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

// Validate keep counts: must be positive integers. A bad value (e.g. "abc")
// would produce NaN, and slice(0, NaN) === [] → every release in that pool
// deleted. Fail-safe to the documented defaults instead.
function positiveIntEnv(name, fallback) {
  const raw = process.env[name] ?? String(fallback);
  if (!/^\d+$/.test(raw) || Number(raw) < 1) {
    console.error(
      `Invalid ${name} value: "${process.env[name]}". Must be a positive integer. Defaulting to ${fallback}.`,
    );
    return fallback;
  }
  return Number(raw);
}

const STABLE_KEEP = positiveIntEnv('STABLE_KEEP', 5);
const PRE_KEEP = positiveIntEnv('PRE_KEEP', 5);

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

/**
 * Pure retention planning (U2 regression-testable): given release records
 * `{ tagName, isDraft }`, split stable/prerelease pools and compute what
 * survives. Exported for tests; the live path calls it below.
 */
export function planPrune(
  releases,
  { stableKeep = STABLE_KEEP, preKeep = PRE_KEEP } = {},
) {
  const versioned = releases.filter((r) => VERSION_RE.test(r.tagName) && !r.isDraft);
  const stable = versioned
    .map((r) => r.tagName)
    .filter((t) => !t.includes('-'))
    .sort((a, b) => compareSemver(a, b) * -1);
  const prerelease = versioned
    .map((r) => r.tagName)
    .filter((t) => t.includes('-'))
    .sort((a, b) => compareSemver(a, b) * -1);
  const keep = new Set([...stable.slice(0, stableKeep), ...prerelease.slice(0, preKeep)]);
  const delete_ = versioned.map((r) => r.tagName).filter((t) => !keep.has(t));
  return { stable, prerelease, keep: [...keep], delete_ };
}

// Live path — guarded so importing this module (tests) never runs it.
if (import.meta.url === `file://${process.argv[1]}`) {
  // 1. List all releases as JSON. Draft flag comes along so in-flight drafts
  //    can be excluded from pruning entirely.
  const raw = gh('release', 'list', '--limit', '200', '--json', 'tagName,isDraft');
  const releases = JSON.parse(raw);

  // 2. Split into separate pools. A prerelease tag (contains '-') can never
  //    displace a stable one, and vice versa: each pool keeps its own N.
  const { stable, prerelease, delete_ } = planPrune(releases);

  console.log(
    `Release prune: ${stable.length} stable / ${prerelease.length} prerelease releases found.`,
  );
  console.log(`Keeping ${STABLE_KEEP} newest stable: ${stable.slice(0, STABLE_KEEP).join(', ')}`);
  console.log(`Keeping ${PRE_KEEP} newest prerelease: ${prerelease.slice(0, PRE_KEEP).join(', ')}`);
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

}
