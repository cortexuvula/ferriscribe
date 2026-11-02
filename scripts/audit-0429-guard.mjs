#!/usr/bin/env node
// Guard for the temporary cargo-audit ignore of RUSTSEC-2026-0429
// (zip <9.0.0: `ZipArchive::extract` aborts on a symlink entry with a huge
// declared ZIP64 size — DoS only, no memory unsafety).
//
// `--ignore RUSTSEC-2026-0429` is advisory-ID-wide: it would also mask any
// NEW pre-9 zip copy that later enters the graph. This script constrains the
// ignore to the one owner-approved residual by inspecting the ACTUAL resolved
// workspace graph (cargo metadata --locked, which includes target-specific
// deps for every platform):
//
//   The only zip <9.0.0 in the graph, on any target, may be exactly
//   zip 8.6.0, and its only consumer may be exactly docx-rs 0.4.22.
//   (docx-rs never calls ZipArchive::extract anywhere in its source; this
//   app uses it writer-only. A docx-rs release with zip >=9 removes the
//   residual entirely.)
//
// Owner approval: Andre, 2026-11-02 ("update the dependencies and allow a
// temporary, documented exception for the unused vulnerable code; review and
// release"). Advisory: https://rustsec.org/advisories/RUSTSEC-2026-0429
// Upstream docx-rs zip>=9 upgrade (unreleased as of 2026-11-02):
// https://github.com/bokuweb/docx-rs/pull/944
// Full rationale: docs/security/RUSTSEC-2026-0429-exception.md
//
// Exit codes (fail-closed — anything unexpected blocks the audit step):
//   0  accepted residual: graph matches the documented exception exactly
//   1  NOT covered: pre-9 zip copy/version/consumer outside the exception,
//      or malformed evidence (metadata unparseable / structure missing)
//   2  residual ELIMINATED: no zip <9.0.0 left — remove the 0429 ignore
//      from ci.yml/audit.yml and this guard (removal trigger fired)
//   3  review window expired (REVIEW_BY below) — reauthorize or remove

import { spawnSync } from 'node:child_process';

// The review-by date is ENFORCED here, not just documented: past this date
// the guard exits 3 and the audit gate fails until an owner-approved commit
// either removes the exception (residual gone) or extends this constant.
const REVIEW_BY = '2026-12-02';

const ACCEPTED_ZIP_VERSION = '8.6.0';
const ACCEPTED_CONSUMER = { name: 'docx-rs', version: '0.4.22' };

function majorOf(version) {
  const m = /^(\d+)\./.exec(version ?? '');
  return m ? Number(m[1]) : NaN;
}

/**
 * Assess the resolved dependency graph against the documented 0429 residual.
 * Pure function over a `cargo metadata --format-version 1` JSON object so the
 * tests can inject fixtures; `now` is a Date for the review-window check.
 * Returns { code, message }.
 */
export function assessResidual(metadata, now) {
  if (!metadata || typeof metadata !== 'object') {
    return { code: 1, message: 'malformed evidence: metadata is not an object' };
  }
  const packages = metadata.packages;
  const resolve = metadata.resolve;
  if (!Array.isArray(packages) || !resolve || !Array.isArray(resolve.nodes)) {
    return { code: 1, message: 'malformed evidence: packages/resolve.nodes missing' };
  }

  const pre9Zip = packages.filter(
    (p) => p && p.name === 'zip' && majorOf(p.version) < 9,
  );
  if (pre9Zip.length === 0) {
    return {
      code: 2,
      message:
        'RUSTSEC-2026-0429 residual eliminated (no zip <9.0.0 in graph): ' +
        'remove the --ignore RUSTSEC-2026-0429 exception and this guard',
    };
  }

  const offenders = pre9Zip.filter((p) => p.version !== ACCEPTED_ZIP_VERSION);
  if (offenders.length > 0 || pre9Zip.length > 1) {
    return {
      code: 1,
      message:
        `RUSTSEC-2026-0429 ignore covers ONLY zip ${ACCEPTED_ZIP_VERSION}; ` +
        `graph has: ${pre9Zip.map((p) => `${p.name} ${p.version}`).join(', ')}`,
    };
  }

  const zipId = pre9Zip[0].id;
  const byId = new Map(packages.map((p) => [p.id, p]));
  const dependents = resolve.nodes
    .filter((n) => Array.isArray(n.deps) && n.deps.some((d) => d && d.pkg === zipId))
    .map((n) => byId.get(n.id))
    .filter(Boolean);

  if (dependents.length === 0) {
    return {
      code: 1,
      message: 'zip 8.6.0 present but orphaned (no consumers) — outside the documented exception',
    };
  }
  const unexpected = dependents.filter(
    (p) => p.name !== ACCEPTED_CONSUMER.name || p.version !== ACCEPTED_CONSUMER.version,
  );
  if (unexpected.length > 0) {
    return {
      code: 1,
      message:
        `RUSTSEC-2026-0429 ignore covers ONLY ${ACCEPTED_CONSUMER.name} ` +
        `${ACCEPTED_CONSUMER.version} consuming zip ${ACCEPTED_ZIP_VERSION}; ` +
        `graph also/instead has: ${dependents.map((p) => `${p.name} ${p.version}`).join(', ')}`,
    };
  }

  const deadline = Date.parse(`${REVIEW_BY}T23:59:59Z`);
  if (Number.isFinite(deadline) && now.getTime() > deadline) {
    return {
      code: 3,
      message:
        `RUSTSEC-2026-0429 exception review window expired ${REVIEW_BY} — ` +
        'reauthorize (extend REVIEW_BY) or remove the ignore',
    };
  }

  return {
    code: 0,
    message:
      `accepted residual: zip ${ACCEPTED_ZIP_VERSION} consumed only by ` +
      `${ACCEPTED_CONSUMER.name} ${ACCEPTED_CONSUMER.version} (extract() unreachable; ` +
      `writer-only usage) — reviewed by ${REVIEW_BY}`,
  };
}

function main() {
  const proc = spawnSync(
    'cargo',
    ['metadata', '--format-version', '1', '--locked', '--no-deps'],
    { encoding: 'utf8' },
  );
  if (proc.error || proc.status !== 0) {
    console.error(`audit-0429-guard: cargo metadata failed: ${proc.error ?? proc.stderr}`);
    process.exit(1);
  }
  // --no-deps keeps the payload small but drops `resolve`; re-run without it
  // when the resolve graph is absent. The full graph is several MB — the
  // spawnSync default maxBuffer (1 MB) ENOBUFS on it.
  let metadata;
  try {
    metadata = JSON.parse(proc.stdout);
  } catch {
    console.error('audit-0429-guard: malformed metadata (unparseable JSON)');
    process.exit(1);
  }
  if (!metadata.resolve) {
    const full = spawnSync('cargo', ['metadata', '--format-version', '1', '--locked'], {
      encoding: 'utf8',
      maxBuffer: 128 * 1024 * 1024,
    });
    if (full.error || full.status !== 0) {
      console.error(`audit-0429-guard: cargo metadata --resolve failed: ${full.error ?? full.stderr}`);
      process.exit(1);
    }
    try {
      metadata = JSON.parse(full.stdout);
    } catch {
      console.error('audit-0429-guard: malformed metadata (unparseable JSON, full)');
      process.exit(1);
    }
  }
  const verdict = assessResidual(metadata, new Date());
  const tag = verdict.code === 0 ? 'ok' : 'BLOCK';
  console[verdict.code === 0 ? 'log' : 'error'](`audit-0429-guard [${tag}]: ${verdict.message}`);
  process.exit(verdict.code);
}

if (process.argv[1] && process.argv[1].endsWith('audit-0429-guard.mjs')) {
  main();
}
