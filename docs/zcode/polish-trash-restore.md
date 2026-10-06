# Zcode prompt — Polish follow-up on the reversible trash-restore feature (approved for merge)

Worktree: /Users/cortexuvula/Development/rustMedicalAssistant/.worktrees/trash-restore
Branch: feat/reversible-trash-restore (HEAD 2226cbd0a8aa5c76c0e655f2617eaa72d0c3a475)
Commit per item or as one logical commit; verify with the gates after; report back.

These are the four residual suggestions from the final review (Codie). Do exactly these; do not refactor anything else.

## Item 1 (fast-follow — closes a documented-constraint violation) + Item 2 (folds in)
Six toast/error call sites interpolate raw backend error strings. The brief's non-negotiable constraints require sanitized errors before display. Fix: add a tiny `sanitizedErr(err: unknown): string` helper (in `src/lib/utils/` or wherever the existing shared helpers live — pick the established location) that scans the message for a `recording <uuid>` id pattern and keeps that (ids are safe — never patient names/content), otherwise returns the generic "unexpected error" string. Apply it to ALL of these call sites:
- src/lib/pages/RecordingsTab.svelte — the toast at ~line 96 (`Could not restore: ${err}`) and the ~line 210 area (`Failed to move recording to Trash: ${err}`, `Could not restore: ${err}`, `Failed to move recordings to Trash: ${err}`).
- src/lib/components/TrashPanel.svelte — the two `Couldn't restore recording(s): ${err}` sites.
- The `undoMoveAll` catch path in src/lib/stores/recordings.svelte.ts that can throw `'No move-all to undo'` — route its rendering through the same sanitized path so a raw throw never reaches the glass.
Existing dialog surfaces already use static strings — leave them alone. Keep the visible wording otherwise identical to today ('Could not restore: …' etc.) with only the interpolated value sanitized.

## Item 3 (comment reword only — no logic change)
crates/db/src/recordings.rs ~lines 591-595, inside `soft_delete_all`: the `rows == 0 → continue` guard's comment claims a race with "a concurrent single-delete inside the transaction window". That mechanism is effectively unreachable under snapshot isolation (a concurrent writer yields SQLITE_BUSY, not zero rows). Reword to: "defensive: unreachable under snapshot isolation; kept so a zero-row UPDATE can never fire the FTS 'delete' against an already-de-indexed row". Do not change the guard itself.

## Item 4 (one line)
src/lib/components/RestoreByDateDialog.svelte — retry path: when a restore FAILS and the user retries, re-run the count preview (the stored preview may be stale — the purge that caused the mismatch may have happened before the retry). On failure, clear/invalidate the preview so the retry path re-queries `countRecordingsDeletedBetween` before executing.

## Gates (run, report each)
1. npx vitest run (all frontend suites; any toast/store tests touched must pin the new helper)
2. npm run check (svelte-check)
3. cargo test --workspace --lib (for the comment-only change this is a compile sanity check)
4. cargo fmt --all -- --check

## Report
- Files changed + the commit sha(s).
- The exact final wording of the sanitized toasts.
- Gate results per item.
Do not touch anything outside these four items; do not run clippy --all-targets (known pre-existing examples noise in crates/stt-providers/).
