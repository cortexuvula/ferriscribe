# Generation workspace — incomplete implementation / provenance blocker

Base: ca8f357; isolated branch feat/generation-design. Production master untouched by this work.

## Verified recovery after worker timeout
The saved frontend changes have 11 passing focused tests across GenerateControls.test.ts and GenerateTab.test.ts; npm run check reports zero errors/warnings. This does NOT establish completion or a working freshness command. Browser production verification remains required.

## Corrected source premise
- src-tauri/src/commands/generation/soap.rs:198–219 and 300–329: generation input capture is opt-in (`capture_for_training`, default false), best-effort, SOAP-only.
- The production GenerationsRepo::record_generation caller found is SOAP capture; tests seed other scenarios, not production writers for each document.
- referral.rs:48–85 and letter.rs:72–109 generate from SOAP note, not directly from transcript. Freshness must also account for the effective SOAP dependency and document-specific inputs (audience/purpose or physician/specialty/reason), without echoing content.
- Raw transcript + input_context_json existing columns do not by themselves provide default, complete output-bound provenance. Do not enable training capture to make UI freshness work.

## Required backend decision / work
Implement a versioned, output-bound provenance contract for all generation producers. Prefer local metadata fingerprints over new duplicated raw input storage, independent of training capture; persist provenance atomically with its matching output. Backend constructs/normalizes effective inputs using the same functions as generation. The read command returns only per-document stale true/false/null and non-content reason codes. Legacy/missing/corrupt/unbound provenance and failed reads must remain Unknown. Never derive freshness in Svelte or mark fresh merely because generation just succeeded.

Acceptance remains unchanged: unchanged inputs fresh; partial notes edit with other structured fields intact stale; exact effective-input reversion fresh; transcript-only edit stale; switch away/back no cross-recording leakage; per-document regeneration isolated from siblings. Include dependency changes, missing provenance, late read ordering and content-free response assertions. These are NOT yet implemented/verified.

## Current artifact limits
GenerateControls has injectable presentation verdicts and defaults unknown. GenerateTab has no freshness API call yet. The timed-out worker wrote an unimplemented Rust test draft referencing read_freshness; moved under docs as a draft so it cannot masquerade as a completed command or break the test build.

No commit, merge, push or release is authorized by this status artifact. Codie review is required before merge.
