# Local-Only AI URL Allowlist — Design

**Status:** Draft — pending user review
**Date:** 2026-05-15
**Author:** Brainstorming session with Claude Code

## Goal

Convert the "local-only AI providers" rule from a convention into a code-enforced invariant: reject any AI or remote-STT host that does not classify as loopback, RFC1918, Tailscale CGNAT/ULA, link-local, or a non-routable hostname (`*.local` / `*.lan` / `*.internal` / `*.home.arpa`), unless the user has explicitly enabled `allow_public_endpoint` in settings.

## Problem

`CLAUDE.md` declares as a hard constraint: *"Local-only AI providers. Only Ollama and LM Studio. No hosted APIs (OpenAI, Anthropic, etc.) — PHI/HIPAA constraint."* Today this is upheld by the Settings UI and by convention only. Nothing in code prevents a user (or a refactor) from pointing `ollama_host` at `api.openai.com`. The tech-debt audit ([this session](../specs/2026-05-14-record-tab-layout-design.md)'s sibling audit on 2026-05-14) flagged this as the top architectural recommendation:

> Today it's a convention. Add a base-URL allowlist (loopback + RFC1918 + Tailscale CGNAT range) at OpenAiCompatibleClient construction with an explicit `allow_public_endpoint` opt-out, and a regression test that asserts `https://api.openai.com` is rejected by default. Makes CLAUDE.md's promise auditable rather than aspirational.

## Non-goals

- **Hosted-provider TTS (ElevenLabs).** Out of scope. TTS does not transmit PHI by default and is a separate policy question already flagged by the audit.
- **DNS resolution at validation time.** We classify by static rules only — no async resolution, no caching, no time-of-check-vs-time-of-use gaps. Domains that aren't statically recognized as local are treated as `Unknown` and behave like `Public` (blocked by default).
- **Office-server pairing endpoints.** Pairing already constrains itself to LAN / Tailscale by construction. The same `validate_local_endpoint` helper is called for defense in depth, but the pairing flow does not change.
- **`OpenAiCompatibleClient` inner constructor enforcement.** Tests that hardcode a base URL (e.g., `https://api.openai.com/v1` in a unit test) are allowed to bypass — the contract is at the provider layer, not the wire-format layer.
- **Settings UI redesign.** Adds one toggle to the AI/STT panels and an inline warning, but no panel re-layout.

## Hard constraints honored

- **Local-only AI.** This change is what makes the constraint enforceable.
- **No PHI in logs.** The new `tracing::warn!` at provider construction logs the field name and classification only — **never the host string**, which could be a self-hosted internal URL the user prefers not to share in support logs.
- **No telemetry.** No network traffic from this change. Classification is pure-function string matching.

## Decisions captured from brainstorming

| Question | Choice |
|---|---|
| Allowlist scope | Loopback + RFC1918 + Tailscale CGNAT/ULA + link-local + mDNS/`.lan`/`.internal`/`.home.arpa` |
| Failure mode | Hard reject by default; explicit `allow_public_endpoint` opt-out |
| Providers covered | AI providers (Ollama + LM Studio) **and** STT remote. TTS out of scope. |

## Architecture overview

```
crates/core/src/
└── endpoint_policy.rs                NEW
    • enum EndpointKind { Loopback, LanRfc1918, LinkLocal, Tailscale, Ula, Mdns, Public, Unknown }
    • pub fn classify_endpoint(host: &str) -> EndpointKind            (pure, no DNS)
    • pub fn validate_local_endpoint(host: &str, allow_public: bool)
              -> Result<(), EndpointPolicyError>
    • pub fn validate_url(url: &str, allow_public: bool)
              -> Result<(), EndpointPolicyError>                      (thin URL→host wrapper)

crates/core/src/error.rs               MODIFIED — new AppError::InvalidEndpoint variant
crates/core/src/types/settings.rs      MODIFIED — new AppConfig.allow_public_endpoint field

crates/ai-providers/src/lmstudio.rs    MODIFIED — new() and new_with_endpoint() take allow_public
crates/ai-providers/src/ollama.rs      MODIFIED — same
crates/stt-providers/src/remote_provider.rs  MODIFIED — same

src-tauri/src/state.rs                 MODIFIED — pass config.allow_public_endpoint into providers
src-tauri/src/commands/settings.rs     MODIFIED — save_settings validates host fields

src/lib/types/index.ts                 MODIFIED — add allow_public_endpoint: boolean to AppConfig
src/lib/stores/settings.ts             MODIFIED — add default `false`
src/lib/components/settings/Models.svelte  MODIFIED — inline warning + Advanced toggle
src/lib/components/settings/Audio.svelte   MODIFIED — same for stt_remote_host
```

## Component contracts

### `endpoint_policy.rs`

```rust
/// What kind of network destination a host string resolves to under static analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EndpointKind {
    /// 127.0.0.0/8, ::1, or the literal hostname "localhost".
    Loopback,
    /// 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16.
    LanRfc1918,
    /// 169.254.0.0/16 (IPv4) or fe80::/10 (IPv6).
    LinkLocal,
    /// 100.64.0.0/10 (Tailscale CGNAT).
    Tailscale,
    /// fc00::/7 (Unique-Local IPv6, used by Tailscale's IPv6 mesh).
    Ula,
    /// Hostname ending in .local / .lan / .internal / .home.arpa (case-insensitive).
    Mdns,
    /// A parseable IP that doesn't match any private range.
    Public,
    /// A hostname that doesn't match any local pattern. Treated as Public by the
    /// validator — we refuse to silently accept `clinic.example.com` just because
    /// we can't DNS-resolve it.
    Unknown,
}

pub fn classify_endpoint(host: &str) -> EndpointKind;

pub fn validate_local_endpoint(
    host: &str,
    allow_public: bool,
) -> Result<(), EndpointPolicyError>;

pub fn validate_url(url: &str, allow_public: bool) -> Result<(), EndpointPolicyError>;

#[derive(Debug, thiserror::Error)]
pub enum EndpointPolicyError {
    #[error("public endpoints are blocked; host='{host}' classified as {kind:?}")]
    Blocked { host: String, kind: EndpointKind },
    #[error("could not parse '{0}' as a URL")]
    UrlParseError(String),
}

impl From<EndpointPolicyError> for AppError { ... }
```

Algorithm for `classify_endpoint`:

1. Trim surrounding `[ ]` if present (IPv6 URL bracket form).
2. Try `host.parse::<IpAddr>()`. On success, dispatch by range as listed in `EndpointKind` docs.
3. Otherwise lowercase and match by suffix: `localhost` → `Loopback`; ends-with `.local` / `.lan` / `.internal` / `.home.arpa` → `Mdns`.
4. Else → `Unknown`.

Algorithm for `validate_local_endpoint`:

- `Loopback | LanRfc1918 | LinkLocal | Tailscale | Ula | Mdns` → `Ok(())`.
- `Public | Unknown` → `Ok(())` if `allow_public`, else `Err(Blocked)`.

### `AppError::InvalidEndpoint`

New variant:

```rust
#[error("invalid endpoint '{host}' for {field}: public/unknown endpoints are blocked (kind={kind:?}). Enable 'Allow public endpoints' in Advanced settings to override.")]
InvalidEndpoint {
    field: String,        // "ollama_host" | "lmstudio_host" | "stt_remote_host"
    host: String,
    kind: EndpointKind,
},
```

`EndpointPolicyError::Blocked` maps into `AppError::InvalidEndpoint` at the call site that owns the field name (the caller passes `field` because the inner classifier doesn't know which settings key it is validating).

### `AppConfig.allow_public_endpoint`

Added to `crates/core/src/types/settings.rs::AppConfig`:

```rust
/// Opt-out: when true, the AI/STT endpoint allowlist is bypassed. Set this
/// only if you understand that PHI may leave the device.
#[serde(default)]
pub allow_public_endpoint: bool,
```

Mirrored in TS `AppConfig` (`src/lib/types/index.ts`) and the `defaults` block (`src/lib/stores/settings.ts`).

### Provider constructors

```rust
// ai-providers/src/lmstudio.rs
impl LmStudioProvider {
    pub fn new(
        host: Option<&str>,
        allow_public: bool,                       // NEW
        bearer: Option<String>,
        policy: RetryConfig,
    ) -> AppResult<Self> {
        let host_str = host.unwrap_or("localhost");
        validate_local_endpoint(host_str, allow_public)
            .map_err(|e| match e {
                EndpointPolicyError::Blocked { host, kind } => AppError::InvalidEndpoint {
                    field: "lmstudio_host".into(),
                    host,
                    kind,
                },
                EndpointPolicyError::UrlParseError(_) => /* same Blocked-shaped error */ unreachable!(),
            })?;
        // ... existing client construction unchanged
    }
    // same for new_with_endpoint
}
```

Same change in `crates/ai-providers/src/ollama.rs` (`field = "ollama_host"`) and `crates/stt-providers/src/remote_provider.rs` (`field = "stt_remote_host"`).

`set_endpoint(ep: Option<RemoteEndpoint>)` validates `ep.lan` and `ep.tailscale` for defense in depth. If either fails, the call returns the error and the existing endpoint is preserved.

### Settings save

`src-tauri/src/commands/settings.rs::save_settings(config)` validates before persisting:

```rust
for (field, host) in [
    ("ollama_host",     config.ollama_host.as_str()),
    ("lmstudio_host",   config.lmstudio_host.as_str()),
    ("stt_remote_host", config.stt_remote_host.as_str()),
] {
    if host.is_empty() { continue; }  // STT remote may legitimately be empty until stt_mode = Remote
    validate_local_endpoint(host, config.allow_public_endpoint)
        .map_err(|e| match e {
            EndpointPolicyError::Blocked { host, kind } => AppError::InvalidEndpoint {
                field: field.into(),
                host,
                kind,
            },
            EndpointPolicyError::UrlParseError(_) => unreachable!(),
        })?;
}
// existing persistence proceeds
```

### Settings UI

**`src/lib/components/settings/Models.svelte`** and **`Audio.svelte`** add:

1. An inline warning under each host input that classifies as Public/Unknown:
   > ⚠ This is a public-internet address. PHI may leave your device. Enable *Allow public endpoints* in Advanced settings to use this anyway.
2. A single shared "Advanced" disclosure section (placed in `General.svelte` or a new collapsible block in each panel — implementation detail for the plan) with the `allow_public_endpoint` toggle, and a permanent banner across the top of Settings when it's `true`:
   > ⚠ Public AI/STT endpoints are enabled. PHI may leave your device.

A small TS helper `src/lib/utils/endpointPolicy.ts` mirrors a subset of the Rust classifier (loopback / RFC1918 / Tailscale / mDNS) for the inline warning. This is **UI feedback only**; the Rust side is the source of truth.

## Data flow

```
User edits ollama_host in Settings → Models.svelte
  ↓
endpointPolicy.ts.classifyHost('api.openai.com') → 'Public'
  ↓
Inline warning rendered under input
  ↓
User clicks save → settings.updateField('ollama_host', 'api.openai.com')
  ↓
saveSettings(config) → invoke('save_settings', { config })
  ↓
Rust: commands::settings::save_settings(config)
  ↓
validate_local_endpoint('api.openai.com', config.allow_public_endpoint=false)
  → Err(EndpointPolicyError::Blocked)
  → AppError::InvalidEndpoint { field: "ollama_host", host: "api.openai.com", kind: Public }
  ↓
formatError(err) in saveSettings's catch → toast/inline error in UI
  ↓
Config NOT persisted

If user enables allow_public_endpoint=true first:
  validate_local_endpoint('api.openai.com', true) → Ok(())
  → config persisted
  → provider construction at next reinit:
    LmStudioProvider::new('api.openai.com', /*allow_public*/ true, ...)
    → validate_local_endpoint(...) → Ok(())
    → tracing::warn!(field="lmstudio_host", kind="Public", "public AI endpoint allowed by opt-out");
    → client constructed; banner remains visible in UI
```

## Error handling

| Scenario | Behavior |
|---|---|
| User saves with public host, opt-out off | Settings command returns `InvalidEndpoint`; UI shows error next to the field; config unchanged |
| User toggles opt-out off while public host is in config | Detected on next provider construction (app restart / `reinit_providers`); affected provider fails to initialize; visible in app log + Settings indicator |
| `validate_local_endpoint` is called on `""` | Caller (Settings save) skips empty strings explicitly. Provider construction receives `None` and substitutes `"localhost"`, so empty never reaches the classifier. |
| Domain with IPv6 brackets (`[fd00::1]`) | `classify_endpoint` strips outer brackets before `parse::<IpAddr>()` |
| `set_endpoint(ep)` after pairing with a non-LAN/Tailscale address | Pairing already enforces LAN/Tailscale shape; `set_endpoint` validates again for defense in depth and returns `InvalidEndpoint` if violated; the old endpoint is preserved |
| User edits the AppConfig JSON file directly to set a public host while `allow_public_endpoint=false` | Detected at next app start: provider construction fails; the app surfaces a "configuration error" toast and falls back to the default provider state |
| `OpenAiCompatibleClient::new` called from a unit test with `https://api.openai.com/v1` | Allowed. The inner client does not enforce; the contract is at the provider layer. Tests don't go through providers. |

## Testing

### Unit tests for `endpoint_policy.rs` (~25 tests)

Cover every classification branch and boundary:

- **Loopback:** `127.0.0.1`, `127.0.0.99`, `127.255.255.254`, `::1`, `localhost`, `LOCALHOST` (case insensitivity), `Localhost`
- **RFC1918 IPv4 boundaries:**
  - `10.0.0.0` (in), `10.255.255.255` (in), `9.255.255.255` (out → Public), `11.0.0.0` (out → Public)
  - `172.16.0.0` (in), `172.31.255.255` (in), `172.15.255.255` (out → Public), `172.32.0.0` (out → Public)
  - `192.168.0.0` (in), `192.168.255.255` (in), `192.167.255.255` (out → Public), `192.169.0.0` (out → Public)
- **Link-local IPv4:** `169.254.0.1` (in), `169.253.255.255` (out → Public), `169.255.0.0` (out → Public)
- **Link-local IPv6:** `fe80::1` (in), `fec0::1` (out → Public)
- **Tailscale CGNAT:** `100.64.0.0` (in), `100.127.255.255` (in), `100.63.255.255` (out → Public), `100.128.0.0` (out → Public)
- **ULA IPv6:** `fd00::1`, `fd7a:115c:a1e0::1` (typical Tailscale), `fc00::1`, `fe00::1` (out → Public)
- **mDNS / non-routable TLDs:**
  - `myhost.local` (Mdns), `clinic.lan` (Mdns), `box.home.arpa` (Mdns), `server.internal` (Mdns)
  - `nested.thing.local` (Mdns — multi-label local domain still classifies)
  - `not.local.com` (Unknown — `.local` is in the middle, not the suffix)
  - `LOCAL.test` (Unknown — case-insensitive suffix is only matched on real local suffixes)
- **Public:** `8.8.8.8`, `1.1.1.1`, `api.openai.com`, `clinic.example.com`, `api.anthropic.com`, `example.com`
- **`validate_local_endpoint` matrix:** every classification × `allow_public ∈ {true, false}` table-driven (2 × 8 = 16 rows)
- **`validate_url`:** `https://api.openai.com/v1` (Public), `http://192.168.1.42:11434` (LanRfc1918), `http://[fd00::1]:11434` (Ula), invalid URL → `UrlParseError`

### Audit regression test

```rust
#[test]
fn audit_regression_api_openai_com_blocked_by_default() {
    use crate::endpoint_policy::*;
    assert_eq!(classify_endpoint("api.openai.com"), EndpointKind::Public);
    assert!(validate_local_endpoint("api.openai.com", false).is_err());
    assert!(validate_local_endpoint("api.openai.com", true).is_ok());
}
```

### Integration tests for providers

For each of `LmStudioProvider`, `OllamaProvider`, `RemoteSttProvider`:

- `new("api.openai.com", false, ...)` → `Err(InvalidEndpoint { kind: Public, field: "lmstudio_host", host: "api.openai.com" })`
- `new("api.openai.com", true, ...)` → `Ok(_)`
- `new("localhost", false, ...)` → `Ok(_)`
- `new("192.168.1.42", false, ...)` → `Ok(_)` (LAN)
- `new("100.64.0.1", false, ...)` → `Ok(_)` (Tailscale)
- `new("clinic.local", false, ...)` → `Ok(_)` (mDNS)
- `new("clinic.example.com", false, ...)` → `Err(InvalidEndpoint { kind: Unknown, ... })`

For `set_endpoint`:

- `lmstudio.set_endpoint(Some(RemoteEndpoint { lan: Some("api.openai.com".into()), ... }))` → `Err(InvalidEndpoint)`; previous endpoint preserved
- `lmstudio.set_endpoint(Some(RemoteEndpoint { lan: Some("192.168.1.42".into()), ... }))` → `Ok(())`

### Integration test for Settings save

`src-tauri/src/commands/settings.rs` (adding a `#[cfg(test)] mod tests` block if absent):

- `save_settings` with `ollama_host = "api.openai.com"` and `allow_public_endpoint = false` → `Err(AppError::InvalidEndpoint { field: "ollama_host", ... })`; on-disk config unchanged.
- Same payload with `allow_public_endpoint = true` → `Ok(())`; config persisted.
- `save_settings` with `stt_remote_host = ""` (empty) and `stt_mode = Local` → `Ok(())`; empty is allowed.
- `save_settings` with `stt_remote_host = "192.168.1.42"` → `Ok(())`.
- `save_settings` with multiple bad hosts → `Err` for the **first** one encountered (in defined field order); a future improvement could aggregate all violations.

### Frontend

No Vitest component tests added (no Svelte component test framework in repo, per the Training Corpus + Record Tab work). The `endpointPolicy.ts` helper, if implemented as a pure TS function, can have small Vitest unit tests covering the same classifications as the Rust side for parity. **Recommended:** add 8-10 quick unit tests on the TS helper so frontend warnings don't drift from backend enforcement.

### Manual smoke (per `CLAUDE.md`)

- Launch `npm run tauri dev`.
- Settings → AI Models: enter `api.openai.com` in `ollama_host`. Inline warning appears. Save fails with the structured error. Field shows error state.
- Enable Advanced → Allow public endpoints. Save again — succeeds. Banner appears across top of Settings.
- Disable the toggle. The bad host stays in the form. Save fails again.
- Reset to `localhost`. Save succeeds. Warning + banner both clear.
- Repeat for `lmstudio_host` and `stt_remote_host`.

## Open questions

None blocking. Future iterations might:
- Aggregate multiple Settings violations into one error response so all fields are flagged at once instead of just the first.
- Add CIDR-based custom allowlists (e.g., a power user with a non-RFC1918 LAN) — out of scope for v1; the opt-out covers it.
- Track `allow_public_endpoint = true` as a telemetry-style metric inside the app log (not phoned home) so support can ask the user "did you turn this on?" rather than guessing.
- Treat ElevenLabs TTS as part of the same allowlist (the audit's separate flag).

## Implementation order

1. **`endpoint_policy.rs` + tests** — pure module, no dependencies. TDD heavy.
2. **`AppError::InvalidEndpoint` variant** — additive enum case; no caller fallout.
3. **`AppConfig.allow_public_endpoint`** + TS mirror + default value.
4. **Provider constructor signature changes** — `LmStudio`, `Ollama`, `RemoteSttProvider`. Update all call sites in `state.rs` and tests.
5. **`set_endpoint` validation** — defense-in-depth check on the runtime-switch path.
6. **Settings save validation** — in `commands/settings.rs`, with integration tests.
7. **Frontend `endpointPolicy.ts`** + inline warnings in `Models.svelte` and `Audio.svelte`.
8. **Advanced toggle + banner** in Settings.
9. **Manual smoke.**

Each step is independently testable. Steps 1–6 ship security value; 7–8 are UX polish.
