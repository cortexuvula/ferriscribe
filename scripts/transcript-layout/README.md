# Transcript layout regression gate

This is the archived c644d42 review's isolated production-component harness, reduced to the absence and blocked-copy layout contracts and made repository-relative. It imports the current `EditorTab`, `TranscriptView`, recording store, copy helper and `src/app.css`; it never copies production markup/CSS into fixtures. All content is synthetic. Native imports are aliased to failing stubs; permitted save/edit bookkeeping and clipboard writes are captured in memory. Non-loopback requests fail the test.

## Run

After `npm ci`, either:

```
uv run --with playwright==1.58.0 python scripts/transcript-layout/verify.py
```

or install `playwright==1.58.0` in a Python venv and run that venv's Python. The script starts/stops its own loopback Vite server on port 14873. It uses `CHROME_PATH`, if provided; otherwise installed macOS Chrome, otherwise Playwright Chromium (`python -m playwright install --with-deps chromium`). CI provisions the venv and Chromium explicitly in `.github/workflows/ci.yml`; missing prerequisites fail, never skip.

Use `--out <directory>` for durable evidence. Default outputs go under ignored `node_modules/.cache/transcript-layout/evidence`. `--smoke` runs the original 390×800 fold-possible reproduction only. Run Python normally, never with `-O` (assertions are the gate).

## Mandatory matrix

- Blocked: 2 reasons (fold possible and unknown saved evidence) × 3 format versions (absent, 0, 1) × 2 themes × 3 modes (normal, grayscale, forced colors) × 4 viewports (390×800, 360×640, 320×640, 800×800) = 144 cases.
- Absence: completed-with-unassigned / skipped / unknown × both themes × all three modes = 18 cases at 360×640, through the production owner.
- Allowed version-2 Copy positive control: exactly one in-memory clipboard write.

Every blocked case must retain the exact gated reason/remedy, display it without hover, disable Copy and produce zero clipboard writes. Before interaction can auto-scroll anything, measure controls, explanation and every text line against the viewport and clipping ancestors; reject overlaps and failed button hit testing. Then actually open Edit and require the textarea. Light/dark matched captures must differ in surface color and image hash. Save incremental JSON, screenshots, console exceptions and blocked requests even on failure. Counts are asserted, not inferred from screenshot filenames.

The original absence checks remain separate from blocked layout checks; a green absence gate cannot approve blocked geometry. The baseline reproduction in `docs/reviews/issue-111/before` fails for Export Audio right-edge clipping and Edit below the viewport even though document scrollWidth alone misses the defect.

## Scope

This proves synthetic Chromium component layout, not native Tauri/WebKit, database persistence, model attribution quality, actual retranscription/export, OS clipboard, full application chrome or screen-reader behavior. Existing #108 absence-state semantics are not repaired here. Keep functional, layout, native and release verdicts separate.
