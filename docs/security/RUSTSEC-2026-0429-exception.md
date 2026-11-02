# RUSTSEC-2026-0429 — acknowledged residual (temporary, owner-approved)

Advisory: https://rustsec.org/advisories/RUSTSEC-2026-0429 (zip <9.0.0:
`ZipArchive::extract` sizes its symlink-target buffer from the attacker-declared
ZIP64 `uncompressed_size` and calls `Vec::with_capacity` on it — a small crafted
archive aborts the process. DoS only: no memory unsafety, disclosure, or
corruption.)

This is an **acknowledged residual, not a fixed-everywhere claim.**

## What was fixed (2026-11-02, feat/trash-two-day-option)

- `tauri-plugin-updater` 2.14.0 → 2.14.1 — its Windows-only zip 4.6.1 copy is
  gone from the graph entirely.
- `calamine` 0.36.1 → 0.38.0 (crates/processing) — now rides zip 9.
- Result: the only zip <9.0.0 left in `Cargo.lock`, on any target, is
  **zip 8.6.0, consumed only by docx-rs 0.4.22** (medical-export).

## The accepted residual and why it is accepted

- docx-rs 0.4.22 is the newest published release (2026-07-21) and requires
  zip `^8.6.0`; the upstream zip-9 upgrade (bokuweb/docx-rs PR #944) was
  merged 2026-10-17 but is **unreleased**. No published crate version can
  remove this copy today.
- The advisory is specific to `ZipArchive::extract`. docx-rs **never calls
  `extract()` anywhere in its source** (reader uses `by_name`, writer uses
  `ZipWriter`), and this app's usage is **writer-only**
  (crates/export/src/docx.rs). The vulnerable API is not reachable through
  this dependency regardless of input.
- Our own zip callers are on zip 9.0.5 (patched); medical-sharing's manual
  extraction additionally pre-caps the declared-size allocation and
  SHA-256-verifies archives before extraction.

## Guard rails (enforced, not just documented)

`--ignore RUSTSEC-2026-0429` is advisory-ID-wide, so
`scripts/audit-0429-guard.mjs` runs before every cargo audit (ci.yml security
job and the scheduled audit.yml workflow) and fail-closes the ignore to this
exact situation, computed from `cargo metadata --locked` (all targets):

- exit 0 — graph matches the residual above;
- exit 1 — any other pre-9 zip version, any additional consumer (including a
  different docx-rs version), an orphaned copy, or malformed evidence;
- exit 2 — **no zip <9 left: the ignore (and guard) must be removed**;
- exit 3 — past the review date below, even if the graph still matches.

Regression tests: `node --test scripts/audit-0429-guard.test.mjs` covers the
accepted residual, new-consumer, malformed-evidence, removed-residual, orphan,
and expiry cases.

## Owner decision and review

- Owner: Andre, 2026-11-02 — "update the dependencies and allow a temporary,
  documented exception for the unused vulnerable code; review and release."
- Review-by: **2026-12-02** — enforced by the guard (exit 3), not a hope.
  Extending it requires an owner-approved commit changing `REVIEW_BY`.
- Removal trigger: a docx-rs release with zip >=9 (upstream PR #944). When it
  lands, bump docx-rs; the guard will then exit 2 until the `--ignore
  RUSTSEC-2026-0429` entries (and the guard itself) are removed from
  ci.yml/audit.yml.
- The pre-existing zip ignores (RUSTSEC-2026-0367/0368/0369, extraction-
  hardening issues in the same docx-rs-pinned 8.6.0 copy) carry their own
  rationale in ci.yml; since 2026-11-02 they too are pinned only by docx-rs.
- A new affected consumer that appears later (any new pre-9 zip dependent) is
  NOT covered by this exception — the guard blocks the audit until it is
  assessed and separately authorized.
