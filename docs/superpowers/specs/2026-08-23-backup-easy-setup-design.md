# Backup easy-setup design (non-technical users)

Date: 2026-08-23
Status: draft — awaiting user review
Scope: making backup **setup** achievable for non-technical users. In-app **restore** is the recommended follow-up spec (see Out of scope).

## Problem

Ferriscribe's backup engine is strong (encrypted content-addressed snapshots,
self-verifying escrow, per-run restore drills, launchd scheduling that survives
the app), but setup requires technical skills at exactly one point: the
off-machine copy. The only supported off-machine destination is a
self-hosted HTTP agent (`ferriscribe-backup serve`) that must be provisioned
on a second machine by hand. A non-technical clinician cannot do this, so they
either decline (data stays machine-local, shares fate with the disk) or accept
a local-only schedule that looks like protection but isn't. Secondary
friction: the Settings → Backup pane is a developer-facing form (typed paths,
bare number inputs, "FERRISCRIBE_BACKUP_APPEND_TOKEN" as a label), the
onboarding step deep-links instead of guiding, nothing validates the
destination at setup time, and escrow print/copy discipline is unassisted.

## Goal

A non-technical user reaches a green "your data is protected" state in a few
minutes, without Terminal, choosing a destination they actually own:

1. A **USB / external / network folder** destination — zero infrastructure.
2. A **cloud-synced folder** (iCloud Drive / Dropbox) — the app writes
   encrypted snapshots into a local folder the OS already syncs.
3. The existing **backup server (agent)** remains the advanced option.

## Assumption (needs user confirmation)

The destination question was asked but unanswered at design time. This spec
assumes **"support both"**: folder destinations as the easy path, agent stays
the advanced path, one wizard serving both. This is the only option that
serves non-technical users without weakening the append-only model for users
who can run the agent.

## Current state (summary)

- `crates/backup/src/job.rs` — `JobConfig { target: Option<BackupTarget> }`
  where `BackupTarget = { url, token }` (HTTP only). Job: build → push →
  re-pull from target → drill → local retention → status write.
- `crates/backup/src/client.rs` — CAS push (HEAD-then-PUT per blob, commit
  with receipt + blob list) and verified pull (HMAC + per-blob hash before
  writing).
- `crates/backup/src/agent.rs` — the target's on-disk store is a **plain
  directory tree**: `<root>/blobs/<xx>/<hash>` shared CAS blobs plus
  `<root>/<snap-id>/{receipt.json, manifest.json.enc, blobs.idx, .committed}`.
- `schedule.rs` — launchd plist pointing a stable tool copy at
  `backup-and-push --url --token` (macOS only).
- Settings → Backup (`Backup.svelte`) — status card + escrow form + schedule
  form; text inputs throughout; no folder pickers, no connectivity test.
- Onboarding `StepBackup.svelte` — text + deep-link to the settings pane.
- `RecordTab.svelte` banner — shows while `!wrappingKeyPresent` (sticky
  dismiss).

## Design

### A. Engine: folder destinations (`medical-backup`)

**Generalize the target.** Replace `BackupTarget { url, token }` with:

```rust
pub enum BackupTarget {
    Agent { url: String, token: String },
    Folder { path: PathBuf },   // absolute, user-chosen, must exist
}
```

**Folder store = agent store layout, written via the filesystem.** A folder
destination holds exactly what `serve --root` would hold: `blobs/<xx>/<hash>`
content-addressed blobs and `<snap-id>/` committed-snapshot dirs. This
deliberate layout parity means:

- every existing invariant (no PHI in filenames — only hashes, ids, sizes;
  fail-closed HMAC verification; `.committed` marker) applies unchanged;
- restore/drill code paths can assemble a snapshot from either store kind
  with one shared reader.

New module `store.rs` (~the filesystem half of the agent's logic):

- `push_to_folder(snapshot_dir, store_root, recordings_dir, wrapping_key)` —
  for each manifest blob: copy to `blobs/<xx>/<hash>` if absent (temp +
  fsync + rename, mirroring the agent's atomicity); write the snapshot dir
  (receipt, manifest, `blobs.idx`, `.committed`). Reports `PushStats`
  (uploaded / skipped) like the HTTP client.
- `assemble_from_folder(store_root, snap_id, out_dir, wrapping_key)` — the
  pull-side: read receipt + manifest, stream each blob from the CAS store,
  verify per-blob SHA-256 and the receipt HMAC **before** writing, and emit a
  plain snapshot dir. Used by the job's re-pull drill and (later) in-app
  restore and the CLI (`restore --store-dir`, `drill --store-dir`).
- `prune_folder_store(store_root, keep_n)` — snapshot dirs are deleted down
  to newest `keep_n` and unreferenced blobs are GC'd. **Folder destinations
  are writable by the source machine, so append-only ransomware protection
  does not apply to them** — this is the accepted trade-off for the easy
  path and is stated in the wizard's UI copy. Default `keep_n = 30`.

**Job changes** (`job.rs`): match on the target enum — Agent path unchanged;
Folder path uses `push_to_folder`, drills by assembling from the folder store
(same temp-dir discipline), and applies `prune_folder_store` after a passing
drill. `pushed_to` records the folder path. Destination-not-present
(folder missing/unmounted) is a **distinct, first-class failure**: a new
`destination_missing: bool` on `BackupRunStatus` with friendly copy
("Backup drive not connected") rather than a raw IO error.

**Schedule** (`main.rs`, `schedule.rs`): `backup-and-push` and
`install-schedule` accept `--dest <path>` as the alternative to
`--url/--token`; the plist bakes whichever was configured. Flags are
strictly additive — existing agent plists and commands keep working
verbatim. When the destination is under `/Volumes`, the plist additionally
sets `StartOnMount` so plugging the drive in triggers a catch-up run (the
job lock serializes; an occasional extra ~30 MB snapshot is accepted and
pruned by retention).

**Config & status.**
- `AppConfig` (crates/core settings): add `backup_dest_path: Option<String>`.
  The two shapes are mutually exclusive; the install command rejects both
  being set.
- `BackupStatus` (Tauri command): add `destination: {kind: 'agent'|'folder'|'local-only', present: bool}` so the UI can say "drive not connected since…".
- New command `backup_test_destination(path)` — existence + write-probe +
  free-space check, used by the wizard at selection time (agent URLs keep
  the current fail-at-run behavior but gain the same wizard test via a
  one-shot `GET /health`-style probe reusing `BackupClient`).

### B. Frontend: one guided wizard (Settings + onboarding share it)

New component `BackupWizard.svelte` (runes mode) with four steps; the
Settings → Backup pane embeds it above the status card (the existing forms
remain reachable under "Advanced options", collapsed), and
`StepBackup.svelte` renders the same component so onboarding finally guides
instead of deep-linking.

1. **Where should backups live?** Three big cards: *External drive*,
   *Folder on this Mac or network* (both open the native folder picker —
   `@tauri-apps/plugin-dialog` is already installed and used elsewhere),
   *Backup server (advanced)* (URL + token fields, existing semantics).
   The chosen folder is validated immediately via `backup_test_destination`
   (writable, plus a free-space readout; approximate first-backup size =
   recording-library size, shown as guidance). Picking a folder under
   iCloud/Dropbox shows a small note: "This folder syncs to the cloud — your
   backups are encrypted and contain no patient-identifying filenames."
2. **Recovery key.** Escrow generation runs with the output folder
   preselected to the user's Desktop (changeable via picker). After writing,
   verification runs **automatically** (the manual verify buttons go away),
   and the step ends with two actions and a checkbox gate: *Print the
   recovery sheet* (reveal in Finder + open with the default text app /
   OS print), *Save USB copy* (reveals the `.escrow` file), and "I've put
   the sheet somewhere safe off this machine" — required to proceed.
3. **When?** A real `type="time"` input (default 03:30), plain-language hint
   that missed times run when the Mac wakes. Install button shows the
   outcome inline.
4. **First backup + test.** Runs the job now with the live event log
   rendered in plain language, ending in "✓ Your data is backed up and a
   test restore passed." or a specific failure with a next action
   ("Connect your backup drive, then try again").

Wizard completion criteria (what clears guidance UI): an off-machine
destination configured **and** last drill passed.

**Honest protection status.** `RecordTab` banner logic changes from
`!wrappingKeyPresent` to `!(off_machine_destination && last_drill_passed)`
(sticky dismiss preserved). The Settings status card states "Backing up to
this Mac only — a disk failure still loses everything" whenever no
off-machine destination exists, so local-only can't masquerade as protected.

### C. Not changing (guarded)

Snapshot format v3, escrow artifact format, key hierarchy, the append-only
HTTP agent and its protocol, CLI surface (flags are additive), no PHI in
logs/artifacts, no remote endpoints contacted by the app (a cloud-synced
folder is the user's OS syncing a local directory — the app writes only to
a user-chosen local path). No cloud-provider APIs. Linux scheduling stays
CLI/README. TTS/hosted-AI constraints untouched.

## Alternatives considered

1. **Wizard around the existing agent only** — smallest effort, but the
   second-machine/server requirement remains; non-technical users still
   cannot complete setup. Rejected as the primary path.
2. **Folder destinations + settings-pane polish only (no wizard)** —
   moderate effort, keeps forms; still demands the user understand ordering
   and jargon ("escrow", "append token"). Rejected: the wizard is where most
   of the "non-technical" win comes from.
3. **Chosen: both** — folder destinations in the engine + one shared wizard
   + honest status. Largest but decomposable (engine and wizard phases land
   independently; see Rollout).

## Error handling

- Destination missing at job time → `destination_missing` status + friendly
  banner/status copy; job still records a red (not stale) state.
- Unwritable/full destination → caught by the wizard probe up front and by
  the job's push step otherwise; probe reports free space so disk-full is
  foreseeable.
- Both `--url` and `--dest` configured → rejected at install and in the CLI
  arg parser with an explicit message.
- Agent reachability failures keep current behavior but the wizard's probe
  surfaces them at setup time instead of at 3 a.m.
- All new error strings are PHI-free (paths, counts, HTTP codes only).

## Testing

- **Rust unit tests** (`crates/backup`): folder push is byte-identical in
  layout to an agent store (reuse the agent test layout helpers); absent
  blob → copied, present blob → skipped; assemble verifies and fails closed
  on a flipped blob byte; prune keeps newest N and GCs only unreferenced
  blobs; job with Folder target drills from the store; destination-missing
  produces `destination_missing`; schedule plist renders `--dest` and
  `StartOnMount` for `/Volumes` paths; mutual-exclusion validation.
- **Frontend (vitest, jsdom):** wizard step gating (can't finish without
  escrow confirmation), folder-picker → probe → error copy, banner clears
  only on off-machine + drill-passed, prefill never clobbers edits.
- **Gates:** `cargo test --workspace --lib`, `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`, `npm run check`,
  `npx vitest run`.

## Rollout

- Phase 1 (engine): target enum, `store.rs`, job/schedule/status/config
  changes — independently shippable, no UI change required.
- Phase 2 (wizard): `BackupWizard.svelte`, Settings embed, onboarding embed,
  banner honesty, `backup_test_destination`.
- Follow-up spec (recommended next): in-app restore wizard
  (pick source → pick/type escrow → verify → guarded apply → restart
  prompt), wrapping `assemble_from_folder` + `restore_snapshot` in Tauri
  commands.
