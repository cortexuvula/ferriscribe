# soap_generator.rs split — design

**Status:** approved 2026-05-08
**Goal:** split `crates/processing/src/soap_generator.rs` (1153 lines) along its existing section banners into a directory module, without changing behavior or breaking call sites.

## Why now

Audit flagged the file as >800 lines. Closer look: ~620 production lines + ~520 lines of tests, already organized with clear section banners. The structure isn't tangled — but four cohesive concerns share one file, and the file has crossed the size where grepping for one of them surfaces noise from the other three. The smaller, well-bounded modules will also be easier to extend (new SOAP templates, new postprocessing rules) without scrolling past unrelated code.

## Final shape

```
crates/processing/src/soap_generator/
├── mod.rs              SoapPromptConfig + re-exports of the 4 public functions
├── prompt_template.rs  default_soap_prompt + build_soap_prompt
│                       + soap_placeholders + icd_code_parts + template_guidance_text
├── user_prompt.rs      build_user_prompt + sanitize_prompt + MAX_CONTEXT_LENGTH
└── postprocess.rs      postprocess_soap + clean_text + format_soap_paragraphs + SECTION_HEADERS
```

## Public API (unchanged)

`mod.rs` re-exports exactly what's used externally today:

| Symbol | Re-exported from |
|---|---|
| `SoapPromptConfig` | declared in `mod.rs` |
| `default_soap_prompt` | `prompt_template` |
| `build_soap_prompt` | `prompt_template` |
| `build_user_prompt` | `user_prompt` |
| `postprocess_soap` | `postprocess` |

Consumers (`src-tauri/src/commands/generation.rs:16`, `src-tauri/src/commands/settings.rs:64`) continue to import from `medical_processing::soap_generator::*`. **Zero call-site changes.**

## Visibility tightening

Three currently-`pub` helpers are not used outside the module today and become `pub(super)` after the split, retaining cross-submodule access without committing to a wider API surface:

- `sanitize_prompt`
- `clean_text`
- `format_soap_paragraphs`

If a future consumer needs them, promoting back to `pub` and adding a re-export is one line each.

## Tests

The existing 30 test functions in the bottom-of-file `#[cfg(test)] mod tests` block are partitioned into three new test modules colocated with their target code:

- `prompt_template::tests` — anything that asserts on system-prompt content / ICD handling / template guidance.
- `user_prompt::tests` — anything that asserts on `sanitize_prompt` or `build_user_prompt` output.
- `postprocess::tests` — anything that asserts on `clean_text`, `format_soap_paragraphs`, or `postprocess_soap`.

Same assertions, same coverage. No test logic changes.

## Execution strategy

The work is one logical move; doing it as one atomic git change avoids transient broken states. After the split:
1. `cargo build -p medical-processing` clean.
2. `cargo test -p medical-processing` — same 30 tests, all green.
3. `cargo test --workspace --lib` clean.
4. `cargo clippy -p rust-medical-assistant --no-deps -- -D warnings` clean (consumers unchanged but verifying anyway).

## Out of scope

- No prompt content changes.
- No logic refactors inside any of the moved functions.
- No new types or abstractions.
- No changes to other large files (`commands/generation.rs:843`, etc.) — those are separate decisions.

## Versioning

Patch bump to `0.10.43`. Pure structural refactor, no user-visible change.
