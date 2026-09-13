# c644d42 — transcript render gate evidence

## Verdict and provenance

PASS for the requested **visual failed-state distinction** and **UI marker-bypass closure**, within the isolated browser scope below. The original 18 absence-state/theme/mode combinations retain distinct wording and have no document overflow. Existing findings #4 and #5 remain unchanged under issue #108; this report does not reopen them as new defects.

Important qualification: the failure has a distinct **visual heading**, but not an HTML/ARIA heading. Do not interpret visual sign-off as a screen-reader-heading or full accessibility sign-off.

- Exact commit: `c644d4291f19198eb4256aaac8313d55b2c6de3e`.
- Only production source consulted: `/Users/cortexuvula/Development/rustMedicalAssistant/.worktrees/copy-metadata-gate`. No production source from copy-epistemic or another stale worktree was used. Prior audit harness/report artifacts were read to recover the baseline procedure.
- Target HEAD remained pinned and target Git status was clean before and after review. No target code/config edits, commits, branch switches, pushes or deployment.
- A `git archive` of the target populated this separate review directory. All 309 archived source/config files were byte-compared with target Git objects after testing. Manifest: `review/evidence/provenance.json`.
- Actual production `EditorTab`, `TranscriptView`, recording store, copy helper and `src/app.css`, compiled by Svelte/Vite. Not an HTML recreation and not jsdom for render evidence.
- Isolated headless installed Chrome **153.0.8010.36**, DPR 1; loopback-only HTTP. Tauri imports aliased to explicit test boundaries before mounting. No patient data, clinical files, live database, audio, providers, user browser profile or real clipboard accessed. No page errors, unexpected IPC calls or external request attempts in the browser matrix.

## 1. Failed versus completed-with-unassigned

### Fixture and clinician-visible result

Identical transcript in both states: `Synthetic unchanged plain passage.` Identical structured segment array: one `speaker: null` span, start 0, end 1, with that same text. Only the persisted outcome changes; the identical `provider_error` reason in metadata is ignored for the completed outcome by the production parent.

**Failed:** the automatic/unverified-label caveat; a full-width banner headed **Speaker labelling failed**; supporting line **Speaker labelling errored during this transcription**; Edit; then the separate **Speaker unassigned** heading and unchanged body. The failure banner has a solid danger rail. The body still has its neutral dashed unassigned rail.

**Completed-with-unassigned:** the same caveat, Edit, and **Speaker unassigned** section/body, but **no failure banner**. It does not display an affirmative “Completed” heading. Successful completion is not independently announced by visible text; the requested pair is nevertheless distinguishable because only failure gets the explicit failure statement and banner.

**Wording: yes, distinct. Visual heading: yes, distinct. Visual salience: yes, distinct in every tested mode.** Neither distinction depends on red alone. The heading/reason/banner remain present in grayscale and forced colors; the solid failure rail remains distinct from the dashed section rail.

Tested 24 renders: 2 outcomes × 2 themes × 3 modes × widths 360 and 800, height 800. Every failed/completed pair had different transcript-view text and different screenshot hashes. Corresponding light/dark screenshot hashes also differed.

### Computed style and contrast

`TranscriptView.svelte:323–331,585–608` supplies the failure banner; `364–366,449–465` supplies the unassigned section/heading. `EditorTab.svelte:138–153,636` transports the saved failure reason.

Common computed geometry at width 800:

- Failure heading: **13px / weight 600 / line-height 19.5px**.
- Reason: **12px / weight 400 / line-height 18px**.
- Failure banner: **3px solid left rail**, **10px 12px padding**, 61.5px high.
- Unassigned heading: **11px / weight 600 / line-height 16.5px**.
- Unassigned section: **3px dashed left rail**, 12px left padding; ordinary body text **14px / line-height 22.4px**.

Contrast ratios below are calculated with WCAG relative luminance from browser computed colors and ancestor-composited backgrounds. Grayscale uses the browser's active `grayscale(1)` transformation applied to those colors; solid-rail/background screenshot pixels were independently sampled to confirm that the filter was actually painted. Values are not antialiased glyph-pixel contrast estimates.

| Theme / mode | Failure heading | Failure reason | Failure rail vs banner | Unassigned heading | Unassigned rail vs body surface |
|---|---:|---:|---:|---:|---:|
| Light / normal | 14.63:1 | 7.76:1 | 4.28:1 | 8.18:1 | 1.30:1 |
| Dark / normal | 8.48:1 | 4.84:1 | 5.44:1 | 5.53:1 | 1.51:1 |
| Light / grayscale | 14.65:1 | 7.77:1 | 6.94:1 | 8.19:1 | 1.30:1 |
| Dark / grayscale | 8.48:1 | 4.85:1 | 4.40:1 | 5.53:1 | 1.51:1 |
| Light / forced | 21:1 | 21:1 | 21:1 | 21:1 | 21:1 |
| Dark / forced | 21:1 | 21:1 | 21:1 | 21:1 | 21:1 |

Normal light: heading `rgb(33,37,41)`, reason `rgb(73,80,87)`, rail `rgb(224,49,49)`, banner `rgb(248,249,250)`. Normal dark: heading `rgb(193,194,197)`, reason `rgb(144,146,150)`, rail `rgb(255,107,107)`, banner `rgb(37,38,43)`.

Grayscale screenshot rail/background samples: light `(86,86,86)/(249,249,249)`; dark `(138,138,138)/(38,38,38)`. Forced-colors samples are black on white in light and white on black in dark, while 3px solid/dashed styles survive. Normal body text contrast is 15.43:1 light and 9.67:1 dark.

The neutral unassigned rail is deliberately reported as weak: **it does not meet 3:1 in normal/grayscale**. The unassigned words remain readable, and the extra failure banner/heading/solid rail clearly separates the states. This gate does not claim the dashed rail alone is a sufficient high-contrast cue.

### At-least-equal-prominence check

Against the actual skipped banner in all six theme/mode combinations, failure has equal heading/reason typography and text contrast, equal padding, and equal 3px solid rail width. Its rail contrast is greater than or equal to skipped: light 4.28 vs 4.10; dark 5.44 vs 4.11; grayscale light 6.94 vs 4.66; grayscale dark 4.40 vs 3.74; forced 21 vs 21.

This establishes equal status typography and at-least-equal rail emphasis, **not equal total occupied area**. The skipped banner is 97.5px high because it additionally includes Open Audio settings; failure is 61.5px and has no corresponding action. Source: `TranscriptView.svelte:536–608`.

### Saved reason and heading semantics

Four browser reason probes passed through the real parent: `provider_error` gives the specific transcription-error wording; missing, unrecognized synthetic code and wrong-domain `models_unavailable` all give **Speaker labelling failed for this recording**. No model-availability, overlap, low-confidence or provider-detail cause is invented. Source: `EditorTab.svelte:143–153`; `TranscriptView.svelte:168–173`.

The visual failure heading is a `<p class="failed-heading">`, not an h1–h6 or role=heading, and its banner has no status/live-region role. The shared unassigned label is an h4. Therefore the two states **do not have distinct heading-navigation structures**, although their visible text does differ. Screen-reader announcement behaviour was not exercised.

Screenshots: `review/evidence/distinction-contact-sheet.png` (all six 800px pairs); full files `distinction-{light|dark}-{normal|grayscale|forced}-{360|800}-{failed|completed-with-unassigned}.png`; generic reason variants `reason-*.png`. Computed values: `results.json → distinction/reasons/prominence`, plus `contrast-summary.json`.

## 2. Marker bypass — closed through the real UI handler

Source: `EditorTab.svelte:422–463` calls `copyDecision` on metadata only; `copyLogic.ts:146–163` owns the decision; `EditorTab.svelte:578–582,729–747` renders the inline explanation.

Exercised separately for format version **absent, 0 and 1**, always with `diarization_fold_evidence: fold_possible`:

1. Click Copy on the original synthetic labelled/suspect passage: **Copy blocked**, visible reason/remedy, zero clipboard writes.
2. Wait until the prior six-second block has actually expired and Copy is enabled. Click Edit. Position the caret at the end and type an additional paragraph using keyboard input: `[Speaker unassigned]` followed by `Synthetic unrelated typed paragraph.` The original suspect passage is byte-for-byte unchanged.
3. Click Copy while that draft is on screen: a **fresh block**, zero clipboard writes.
4. Click Done, observe Saved, and verify the exact edited text in the real frontend store and mocked `save_recording_field` payload. Metadata remains unchanged. After the previous block expires, click Copy again: **blocked**.
5. Unmount/remount the production EditorTab over that frontend record and click Copy again: **blocked**.

All **12 block checkpoints** passed. In each version case, zero writes reached the clipboard boundary throughout. A separate version-2 positive control reached the same boundary exactly once and copied the current draft with the caveat, proving that an inert/disconnected Copy control did not manufacture the passing negative results.

The clinician sees **Copy blocked** plus the saved-evidence explanation and **Re-transcribe this recording to get an honest copy**, including the warning that re-transcription may replace manual corrections and does not guarantee correct labels. The message is visible inline, not hover-dependent, at the exercised 800px width.

Evidence: `bypass-{absent|0|1}-{baseline|typed-draft|after-done|remount}.png`, `results.json → bypass/positiveControls`. This is UI-end-to-end **up to mocked persistence/clipboard boundaries**, not a real database restart/reopen test. The mock acknowledges the exact save payload; it does not execute Rust metadata preservation or simulate a database reload.

## 3. Existing issue #108 — no observed shift

Using the user's original finding numbers:

- **#4 remains open and unchanged.** Off/skipped/unknown with two plain paragraphs and absent, empty or invalid segment metadata still manufactures two Speaker unassigned headings plus the automatic-label caveat. Unknown loses its status-unavailable presentation; skipped retains the wasn't-run banner beside the contradictory attempted-attribution presentation. Source: `TranscriptView.svelte:138–152,303–306,349–369`.
- **#5 remains open and unchanged.** Unknown single-paragraph text with those same three segment-metadata conditions still has no Edit action. The status branch precedes the editing and toolbar branches. Source: `TranscriptView.svelte:333–351`.

All 18 paragraph/outcome/metadata observations were checked against these expected existing behaviours. Off/skipped one-paragraph controls still have Edit and no unassigned heading. No new issue filed and no claim that these are repaired. Evidence: `results.json → issue108`; `issue108-*.png`.

## 4. Original 18 absence combinations

Re-ran the exact prior composition: **completed-with-unassigned / skipped / unknown × light/dark × normal/grayscale/forced**, production standalone TranscriptView, one synthetic all-null span, **360×640**. All 18 have the intended distinct wording, three distinct text states, and document scroll dimensions **360×640**, equal to client dimensions. No document overflow on either axis.

Repeated all 18 through production EditorTab: also no document overflow. Expected transcript/scroll regions existed with nonzero geometry; collected regions had no horizontal inner overflow. Files: `absence-reader-*.png`, `absence-editor-*.png`; `results.json → absence/absenceEditor`.

Do not broaden this into “all narrow layout is polished.” At 360px the header's Transcript title and control group have **zero horizontal gap** (both meet at x=89.296875) and button labels wrap. Vision flagged apparent crowding; targeted DOM measurements found no overlapping bounding boxes or document overflow. See `narrow-header.json` and `distinction-dark-forced-360-failed.png`. The failure and unassigned texts themselves remain visible and separate.

## Verification inventory and limits

- Browser run exited 0: 24 distinction renders; original 18 absence renders plus 18 owner renders; 4 reason probes; 6 computed prominence comparisons; 18 existing-issue probes; 3 four-phase bypass sequences; 1 positive control. Results are persisted incrementally and aggregated from JSON, not counted from screenshots alone.
- Additional final evidence assertions: 47 passed, including source integrity, measured contrast thresholds for the failure banner, theme/hash differences, absence dimensions and unchanged #108 behaviour.
- Focused jsdom suite independently re-run: **3 files / 43 tests passed** (`TranscriptView.test.ts`, `EditorTab.copy.test.ts`, `copyLogic.test.ts`). This supplements, not substitutes for, browser CSS evidence. Non-failing Vite config-loader warning is retained in `review/evidence/vitest.txt`.
- Harness compilation also warned that its initialization-only `transcript` and `metadata` locals are not `$state`. They are constructed once from URL parameters before mounting; every matrix transition navigates/recreates the fixture rather than mutating those locals. Edit probes use the production reactive recording store and editor draft. These compile warnings are not page exceptions; no claim of a warning-free build is made. The loopback Vite process was stopped and its exited state verified after evidence collection.
- 89 PNGs including the contact sheet; hashes in `provenance.json`. Visually inspected: the contact sheet containing all twelve 800px distinction screenshots, the absent-version typed-draft block screenshot, and the 360px dark forced failure screenshot. Remaining screenshots were captured and programmatically checked, not individually vision-reviewed.
- NOT tested: native Tauri/WebKit/Windows app or real Windows high-contrast settings; Safari/Firefox; screen reader, live announcements or full keyboard/focus audit; full application navigation; database edit transactions, backend reload/restart, retranscription, repair, audio export or generated-note consumers; real OS clipboard; long transcripts, zoom/enlarged text, performance; whole-repository build, lint, type check, full tests or release/package installation. Grayscale is a browser CSS simulation, not display hardware calibration. No actual diarization/model-quality claim.

## Reproduction

From this review directory, with the target project's existing dependencies available via node_modules:

    node_modules/.bin/vite --config review/vite.config.ts
    uv run --with playwright python review/verify.py
    python3 review/finalize_evidence.py
    node_modules/.bin/vitest run src/lib/components/TranscriptView.test.ts src/lib/pages/EditorTab.copy.test.ts src/lib/utils/copyLogic.test.ts

The harness is bound to 127.0.0.1:14872 and uses only synthetic fixtures. Source archive and evidence are separate from the clean target checkout. The final provenance script deliberately checks the exact original target path/HEAD; another machine must provide that source location or adapt only the harness verification path.
