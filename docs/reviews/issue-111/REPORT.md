# Issue #111 — blocked-copy layout repair

## Verdict

PASS for the requested layout repair in the isolated Chromium harness. Ready for review; not merged, pushed, published or released. No claim of native app or full accessibility approval.

Implementation branch: `fix/111-blocked-copy-layout`, based on master `177144e7a09eee853b6c6568baad283edc5e6a45`.
Worktree: `/Users/cortexuvula/Development/rustMedicalAssistant/.worktrees/ui-111-blocked-copy`.

The evidence captured before committing records the base HEAD and the actual tested `EditorTab.svelte` SHA-256 in `after/results.json`. Use that content hash, not the base HEAD, to identify the repaired source in these captures. An exact-committed-HEAD re-run is retained separately under the reviewer's `reviews/issue-111-final` archive and reported in the handoff.

## Change and preserved contract

The explanation now spans its own full-width row below the wrapping header controls. The title and action group can wrap, and controls no longer shrink into the explanation. The reason/remedy stays inline with its status role, test id and Copy `aria-describedby` association intact; hover is not needed.

Only `EditorTab.svelte` markup/CSS changes in production. Its entire script and `copyLogic.ts` are unchanged. The moved conditional retains the original content-present gate. No change to the decision, timeout, exact reason/remedy wording, clipboard handler, persistence or re-transcription behavior. The hard block remains hard.

## Red → green evidence

Before modification, the new executable geometry gate exited 1 at 390×800:

- Export Audio right edge: **406.45px**, beyond the clipped 390px viewport.
- Edit: **844.06–872.06px** vertically, outside the 800px viewport, not hit-testable.
- See `before/results.json` and `before/blocked-light-normal-390-fold_possible-absent.png`.

After modification, all **144 blocked cases** pass: 2 saved-evidence reasons × 3 format versions (absent, 0, 1) × 2 themes × 3 modes × 4 viewports. Viewports: **390×800, 360×640, 320×640, 800×800**. Modes: normal, grayscale simulation, browser forced colors.

Every case asserts the exact full reason/remedy, visible without hover; disabled Copy; zero clipboard writes; control/text-line viewport and clipping-ancestor bounds; no control/explanation overlap; successful button hit tests; and opening Edit. Geometry is captured before an automated click can scroll the target. Edit opens through pointer interaction in 96 cases and real Tab → Enter traversal in 48 cases. Full light/dark screenshot pairs and computed surface colors differ.

For the long fold-possible explanation in light/normal mode:

| Viewport | Export Audio right | Edit vertical bounds |
|---|---:|---:|
| 390×800 | 332.16px | 341.47–369.47px |
| 360×640 | 332.16px | 341.47–369.47px |
| 320×640 | 116.23px (wrapped row) | 429.86–457.86px |
| 800×800 | 784px | 210.48–238.48px |

The separate **18 absence/theme/mode cases** remain pinned through production EditorTab at 360×640. A version-2 positive control reaches the mocked clipboard exactly once. No browser page exceptions, unexpected native calls or external request attempts were observed. Browser: installed Chrome **153.0.8010.36**, headless, DPR 1, isolated profile, loopback only.

Raw geometry, text-line rectangles, hashes, state, counts and screenshots: `after/results.json` and `after/*.png`. Visually inspected: baseline light/normal 390px, repaired light/normal 320px, repaired dark/forced 390px. Other matrix screenshots are programmatically checked, not individually vision-reviewed.

## Gates

- `npx vitest run`: **99 files / 871 tests passed**. `vitest.log`. Includes existing copy/bypass contract tests.
- `npm run check`: **0 errors, 0 warnings**. `check.log`.
- `npm run lint`: exits 0, **0 errors / 53 existing warnings**, none in the new harness. `lint.log`.
- `npm run build`: exits 0; existing bundle chunk-size warning. `build.log`.
- `git diff --check`: passes.
- Browser gate: **144 blocked + 18 absence + 1 positive control; zero layout failures**.

The Vite native-config-loader warning remains in tooling output. No dependency updates or advisory remediation were attempted; `npm ci --ignore-scripts` reported 24 moderate advisories.

## Preventing recurrence

The runnable harness is `scripts/transcript-layout/verify.py`. Its README documents the matrix and prerequisites. `.github/workflows/ci.yml` explicitly installs pinned Python Playwright and Chromium, then runs the complete layout gate; missing prerequisites fail rather than skip. This CI wiring has not yet run on GitHub because this branch has not been pushed.

Both the original reviewer archive's `REPORT.md` and the checked-in `docs/reviews/render-gate-c644d42/REPORT.md` now start with **overall REQUEST CHANGES for #111**, with a dedicated section and linked failing geometry/screenshot evidence. Passing the two prior functional items is explicitly not an overall green gate. Historical results were preserved; added baseline evidence is labelled a later reproduction.

The c644d42 archive, original harness and evidence were available. No missing artifact from the removed disposable checkout was needed; no restoration of that checkout is required for this repair.

## Unverified / outside scope

Native Tauri/WebKit, Safari/Firefox, Windows or real OS high-contrast behavior; real database save/reload, OS clipboard, audio export, re-transcription and manual-correction replacement; full app shell/navigation, sync-enabled extra toolbar controls, long patient-name headers, long transcripts, zoom/enlarged text, screen readers and performance. Backend Rust gates, release packaging, installed-app smoke test and remote CI have not run. Existing issue #108 remains separate. This change makes existing controls reachable; it does not introduce a new re-transcribe button or claim a tested end-to-end recovery workflow.

## Reproduce

From the branch's worktree after `npm ci`:

```
uv run --with playwright==1.58.0 python scripts/transcript-layout/verify.py --out /absolute/evidence/directory
```

The runner starts and stops its own loopback server. All fixtures are synthetic, all native actions are blocked or recorded in memory, and no patient files, database, live app or actual clipboard are accessed.
