# Recordings retention policy — Design

**Date:** 2026-08-16
**Status:** Approved (approach A, subagent execution)

## Problem

Recordings accumulate forever unless the user manually deletes them. For a
medical practice, an opt-in retention policy (auto-trash recordings older
than N days) supports data-minimization hygiene. The building blocks already
exist: soft-delete trash with undo, `deleted_at` propagation through content
sync, and a server-only tombstone sweeper that permanently purges soft-deleted
recordings (DB row + RAG vectors + encrypted audio) 30 days after deletion.

## Goal

An opt-in, default-off setting: *"Automatically move recordings to trash when
older than N days"* (Never / 30 / 90 / 180 / 365). No new purge machinery —
retention only soft-deletes; the existing trash → sync → purge pipeline does
the rest.

## Design

### Setting

`AppConfig.retention_days: Option<u32>` (`#[serde(default)]`, default `None`
= off). Per-machine (settings are not content-synced). UI lives in
**Settings → Data Management**: a dropdown plus helper text explaining the
lifecycle (trash keeps a 30-day undo before permanent deletion; restoring
exempts a recording from future auto-cleanup).

### Retention sweep (`crates/db`)

New `RecordingsRepo` fn, e.g. `retention_soft_delete_older_than(conn, days, now) -> DbResult<Vec<Uuid>>`:
- Candidates: `deleted_at IS NULL AND created_at < now - days`, with
  `metadata.retention_exempt` absent. Metadata filtering in Rust (fetch
  id/created_at/metadata, filter) — avoids any JSON1 SQL dependence.
- Applies the existing `soft_delete` per candidate and returns the ids (for
  count-only logging).
- `RecordingsRepo::restore` (existing) additionally stamps
  `metadata.retention_exempt = true` (read-modify-write the metadata JSON in
  Rust) so a restored recording is an explicit user override the sweep never
  re-trashes. The flag rides existing metadata sync.

### Sweeper loop (`src-tauri/src/state.rs`)

The daily loop currently spawns only when acting as office server and only
runs the tombstone purge. Restructure: spawn unconditionally; each 24h tick
reloads config from the DB (so runtime setting changes apply without
restart), then:
- if office server → existing tombstone purge (unchanged behavior);
- if `retention_days` is `Some(n)` → `retention_soft_delete_older_than(n)`,
  logging only the count ("retention sweep: moved N recordings to trash").

### Privacy

Sweep logs counts and durations only (AGENTS.md). Soft-deletes propagate via
existing sync; permanent purge continues to happen only on the office server.

## Testing

- `medical-db` integration tests: candidate selection respects age,
  `deleted_at IS NULL`, and the exemption flag; `restore` stamps the
  exemption; re-running the sweep is idempotent (already-deleted stay
  deleted; exempt stay put).
- `medical-core`: AppConfig serde roundtrip with and without the new field
  (old configs default to off).
- Frontend: DataManagement renders the dropdown with Never default; saving
  maps to `retention_days` null vs number; helper text present.
- Gates: fmt, clippy `-D warnings`, `cargo test -p medical-db`, workspace
  lib, vitest, `npm run check`.

## Out of scope

Configurable trash-purge window (stays 30 days); manual "clean now" button;
per-recording retention overrides beyond the restore exemption; changing
server-only purge semantics.
