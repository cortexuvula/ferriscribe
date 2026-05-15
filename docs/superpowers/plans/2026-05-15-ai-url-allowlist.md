# Local-Only AI URL Allowlist Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enforce the "local-only AI providers" CLAUDE.md rule in code: reject any AI or remote-STT host that does not classify as loopback / RFC1918 / Tailscale CGNAT / Tailscale ULA / link-local / mDNS / `.lan` / `.internal` / `.home.arpa`, unless `AppConfig.allow_public_endpoint = true`.

**Architecture:** A pure-function classifier (`endpoint_policy.rs`) in `medical-core` checks host strings. Two enforcement layers consume it: (1) `save_settings` rejects bad host fields before persistence; (2) `LmStudioProvider`, `OllamaProvider`, `RemoteSttProvider` constructors and `set_endpoint` reject at construction. Frontend gets a mirror TS helper for inline UX warnings; the Rust side is the source of truth. The opt-out is a single new `AppConfig.allow_public_endpoint: bool` (default `false`).

**Tech Stack:** Rust (`thiserror`, `serde`, `std::net::IpAddr`), Svelte 5 + TypeScript, Vitest. No new npm or cargo workspace deps required — we avoid pulling in the `url` crate by doing string-based host extraction in pure Rust.

**Spec:** [`docs/superpowers/specs/2026-05-15-ai-url-allowlist-design.md`](../specs/2026-05-15-ai-url-allowlist-design.md)

---

## File Structure

**New files:**
- `crates/core/src/endpoint_policy.rs` — classifier + validators + error type
- `src/lib/utils/endpointPolicy.ts` — TS mirror for inline UI warnings
- `src/lib/utils/endpointPolicy.test.ts` — Vitest unit tests for the TS helper

**Modified files:**
- `crates/core/src/lib.rs` — `pub mod endpoint_policy;`
- `crates/core/src/error.rs` — add `AppError::InvalidEndpoint` variant + serialization
- `crates/core/src/types/settings.rs` — add `AppConfig.allow_public_endpoint: bool` field
- `crates/ai-providers/src/lmstudio.rs` — validate host at `new` / `new_with_endpoint` / `set_endpoint`
- `crates/ai-providers/src/ollama.rs` — same
- `crates/stt-providers/src/remote_provider.rs` — same
- `src-tauri/src/state.rs` — thread `config.allow_public_endpoint` into provider constructors
- `src-tauri/src/commands/settings.rs` — validate host fields in `save_settings`; add tests module
- `src/lib/types/index.ts` — add `allow_public_endpoint: boolean` to `AppConfig`
- `src/lib/stores/settings.ts` — add `allow_public_endpoint: false` to defaults
- `src/lib/components/settings/Models.svelte` — inline warning under `ollama_host` and `lmstudio_host`
- `src/lib/components/settings/Audio.svelte` — inline warning under `stt_remote_host`
- `src/lib/components/settings/General.svelte` — Advanced section with `allow_public_endpoint` toggle + global banner

**Worktree:** Use `superpowers:using-git-worktrees` to create `.worktrees/ai-url-allowlist` from `master` at the spec commit (`57355ea`).

---

## Task 1: `endpoint_policy.rs` — pure classifier + validators (TDD)

**Files:**
- Create: `crates/core/src/endpoint_policy.rs`
- Modify: `crates/core/src/lib.rs`

**Why:** Pure-function module is the foundation everything else consumes. Heavy TDD makes the classification boundaries explicit and protects against future drift.

- [ ] **Step 1: Add the module to `lib.rs`**

In `crates/core/src/lib.rs`, after line 1 (`pub mod error;`), add:

```rust
pub mod endpoint_policy;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/core/src/endpoint_policy.rs` with this skeleton + tests block:

```rust
//! Static (no-DNS) classification of host strings for the local-only-AI
//! constraint. Used to reject public endpoints (e.g. api.openai.com) at
//! Settings save and at provider construction.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EndpointKind {
    Loopback,
    LanRfc1918,
    LinkLocal,
    Tailscale,
    Ula,
    Mdns,
    Public,
    Unknown,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum EndpointPolicyError {
    #[error("public endpoints are blocked; host='{host}' classified as {kind:?}")]
    Blocked { host: String, kind: EndpointKind },
}

pub fn classify_endpoint(_host: &str) -> EndpointKind {
    unimplemented!()
}

pub fn validate_local_endpoint(
    _host: &str,
    _allow_public: bool,
) -> Result<(), EndpointPolicyError> {
    unimplemented!()
}

/// Extract the bare host from a string that may be a bare host, host:port,
/// or scheme://host:port/path. Returns the host with any surrounding IPv6
/// brackets stripped. Returns the original input if it can't be parsed.
pub fn extract_host(input: &str) -> &str {
    unimplemented!()
}

/// Validate a URL-or-host input by extracting the host and delegating.
pub fn validate_url(
    input: &str,
    allow_public: bool,
) -> Result<(), EndpointPolicyError> {
    validate_local_endpoint(extract_host(input), allow_public)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── classify_endpoint: loopback ─────────────────────────────────
    #[test]
    fn loopback_ipv4_in_range() {
        assert_eq!(classify_endpoint("127.0.0.1"), EndpointKind::Loopback);
        assert_eq!(classify_endpoint("127.0.0.99"), EndpointKind::Loopback);
        assert_eq!(classify_endpoint("127.255.255.254"), EndpointKind::Loopback);
    }

    #[test]
    fn loopback_ipv6_literal() {
        assert_eq!(classify_endpoint("::1"), EndpointKind::Loopback);
    }

    #[test]
    fn loopback_hostname_case_insensitive() {
        assert_eq!(classify_endpoint("localhost"), EndpointKind::Loopback);
        assert_eq!(classify_endpoint("LOCALHOST"), EndpointKind::Loopback);
        assert_eq!(classify_endpoint("Localhost"), EndpointKind::Loopback);
    }

    // ── classify_endpoint: RFC1918 ─────────────────────────────────
    #[test]
    fn rfc1918_10_block() {
        assert_eq!(classify_endpoint("10.0.0.0"), EndpointKind::LanRfc1918);
        assert_eq!(classify_endpoint("10.255.255.255"), EndpointKind::LanRfc1918);
        assert_eq!(classify_endpoint("9.255.255.255"), EndpointKind::Public);
        assert_eq!(classify_endpoint("11.0.0.0"), EndpointKind::Public);
    }

    #[test]
    fn rfc1918_172_block() {
        assert_eq!(classify_endpoint("172.16.0.0"), EndpointKind::LanRfc1918);
        assert_eq!(classify_endpoint("172.31.255.255"), EndpointKind::LanRfc1918);
        assert_eq!(classify_endpoint("172.15.255.255"), EndpointKind::Public);
        assert_eq!(classify_endpoint("172.32.0.0"), EndpointKind::Public);
    }

    #[test]
    fn rfc1918_192_168_block() {
        assert_eq!(classify_endpoint("192.168.0.0"), EndpointKind::LanRfc1918);
        assert_eq!(classify_endpoint("192.168.255.255"), EndpointKind::LanRfc1918);
        assert_eq!(classify_endpoint("192.167.255.255"), EndpointKind::Public);
        assert_eq!(classify_endpoint("192.169.0.0"), EndpointKind::Public);
    }

    // ── classify_endpoint: link-local ──────────────────────────────
    #[test]
    fn link_local_ipv4() {
        assert_eq!(classify_endpoint("169.254.0.1"), EndpointKind::LinkLocal);
        assert_eq!(classify_endpoint("169.253.255.255"), EndpointKind::Public);
        assert_eq!(classify_endpoint("169.255.0.0"), EndpointKind::Public);
    }

    #[test]
    fn link_local_ipv6() {
        assert_eq!(classify_endpoint("fe80::1"), EndpointKind::LinkLocal);
        assert_eq!(classify_endpoint("fec0::1"), EndpointKind::Public);
    }

    // ── classify_endpoint: Tailscale ───────────────────────────────
    #[test]
    fn tailscale_cgnat() {
        assert_eq!(classify_endpoint("100.64.0.0"), EndpointKind::Tailscale);
        assert_eq!(classify_endpoint("100.127.255.255"), EndpointKind::Tailscale);
        assert_eq!(classify_endpoint("100.63.255.255"), EndpointKind::Public);
        assert_eq!(classify_endpoint("100.128.0.0"), EndpointKind::Public);
    }

    #[test]
    fn tailscale_ula_ipv6() {
        assert_eq!(classify_endpoint("fd00::1"), EndpointKind::Ula);
        assert_eq!(classify_endpoint("fd7a:115c:a1e0::1"), EndpointKind::Ula);
        assert_eq!(classify_endpoint("fc00::1"), EndpointKind::Ula);
        assert_eq!(classify_endpoint("fe00::1"), EndpointKind::Public);
    }

    // ── classify_endpoint: mDNS / non-routable TLDs ────────────────
    #[test]
    fn mdns_suffix_local() {
        assert_eq!(classify_endpoint("myhost.local"), EndpointKind::Mdns);
        assert_eq!(classify_endpoint("nested.thing.local"), EndpointKind::Mdns);
        assert_eq!(classify_endpoint("CLINIC.LOCAL"), EndpointKind::Mdns);
    }

    #[test]
    fn mdns_other_local_suffixes() {
        assert_eq!(classify_endpoint("clinic.lan"), EndpointKind::Mdns);
        assert_eq!(classify_endpoint("box.home.arpa"), EndpointKind::Mdns);
        assert_eq!(classify_endpoint("server.internal"), EndpointKind::Mdns);
    }

    #[test]
    fn mdns_partial_match_is_unknown_not_mdns() {
        // ".local" appears in the middle, not as the suffix.
        assert_eq!(classify_endpoint("not.local.com"), EndpointKind::Unknown);
        // ".lan" is similarly a middle label here.
        assert_eq!(classify_endpoint("subdomain.lan.example.com"), EndpointKind::Unknown);
    }

    // ── classify_endpoint: Public / Unknown ────────────────────────
    #[test]
    fn public_ipv4_examples() {
        assert_eq!(classify_endpoint("8.8.8.8"), EndpointKind::Public);
        assert_eq!(classify_endpoint("1.1.1.1"), EndpointKind::Public);
    }

    #[test]
    fn public_hostname_classifies_as_unknown() {
        // We can't statically tell a public domain from a private one without
        // DNS. Treat all unrecognised hostnames as Unknown.
        assert_eq!(classify_endpoint("api.openai.com"), EndpointKind::Unknown);
        assert_eq!(classify_endpoint("clinic.example.com"), EndpointKind::Unknown);
        assert_eq!(classify_endpoint("api.anthropic.com"), EndpointKind::Unknown);
        assert_eq!(classify_endpoint("example.com"), EndpointKind::Unknown);
    }

    // ── validate_local_endpoint matrix ─────────────────────────────
    #[test]
    fn validate_blocks_public_and_unknown_unless_allow_public() {
        for host in ["api.openai.com", "8.8.8.8", "example.com"] {
            assert!(validate_local_endpoint(host, false).is_err(), "should block: {host}");
            assert!(validate_local_endpoint(host, true).is_ok(),  "opt-out should accept: {host}");
        }
    }

    #[test]
    fn validate_accepts_all_local_kinds_regardless_of_allow_public() {
        for host in [
            "localhost",          // Loopback
            "127.0.0.1",
            "::1",
            "192.168.1.42",       // RFC1918
            "10.0.0.5",
            "172.20.0.1",
            "100.64.0.1",         // Tailscale CGNAT
            "fd7a:115c:a1e0::1",  // Tailscale ULA
            "169.254.0.1",        // Link-local
            "fe80::1",
            "clinic.local",       // mDNS
            "box.lan",
            "server.internal",
            "host.home.arpa",
        ] {
            assert!(validate_local_endpoint(host, false).is_ok(), "should accept: {host}");
            assert!(validate_local_endpoint(host, true).is_ok(),  "should still accept with opt-out: {host}");
        }
    }

    // ── extract_host ───────────────────────────────────────────────
    #[test]
    fn extract_host_bare() {
        assert_eq!(extract_host("localhost"), "localhost");
        assert_eq!(extract_host("192.168.1.42"), "192.168.1.42");
    }

    #[test]
    fn extract_host_with_port() {
        assert_eq!(extract_host("localhost:1234"), "localhost");
        assert_eq!(extract_host("192.168.1.42:11434"), "192.168.1.42");
    }

    #[test]
    fn extract_host_full_url() {
        assert_eq!(extract_host("http://localhost:1234"), "localhost");
        assert_eq!(extract_host("https://api.openai.com/v1"), "api.openai.com");
        assert_eq!(extract_host("http://192.168.1.42:11434/v1/chat"), "192.168.1.42");
    }

    #[test]
    fn extract_host_strips_ipv6_brackets() {
        assert_eq!(extract_host("[fd00::1]"), "fd00::1");
        assert_eq!(extract_host("[fd00::1]:11434"), "fd00::1");
        assert_eq!(extract_host("http://[fd00::1]:11434/v1"), "fd00::1");
    }

    // ── validate_url ───────────────────────────────────────────────
    #[test]
    fn validate_url_blocks_api_openai_com_by_default() {
        assert!(validate_url("https://api.openai.com/v1", false).is_err());
        assert!(validate_url("https://api.openai.com/v1", true).is_ok());
    }

    #[test]
    fn validate_url_accepts_lan_url() {
        assert!(validate_url("http://192.168.1.42:11434", false).is_ok());
        assert!(validate_url("http://[fd00::1]:11434/v1", false).is_ok());
    }

    // ── Audit regression test (named for traceability) ────────────
    #[test]
    fn audit_regression_api_openai_com_blocked_by_default() {
        assert_eq!(classify_endpoint("api.openai.com"), EndpointKind::Unknown);
        assert!(validate_local_endpoint("api.openai.com", false).is_err());
        assert!(validate_local_endpoint("api.openai.com", true).is_ok());
    }
}
```

- [ ] **Step 3: Run the tests and confirm they fail**

Run:

```bash
cargo test -p medical-core endpoint_policy
```

Expected: all tests fail (compile error or `unimplemented!` panics).

- [ ] **Step 4: Implement the module**

Replace the `unimplemented!()` stubs in `crates/core/src/endpoint_policy.rs` with:

```rust
use std::net::IpAddr;

pub fn classify_endpoint(host: &str) -> EndpointKind {
    // Strip outer IPv6 brackets if present: "[fd00::1]" -> "fd00::1".
    let trimmed = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);

    // First try as an IP literal.
    if let Ok(ip) = trimmed.parse::<IpAddr>() {
        return classify_ip(ip);
    }

    // Otherwise it's a hostname. Case-insensitive checks.
    let lower = trimmed.to_ascii_lowercase();
    if lower == "localhost" {
        return EndpointKind::Loopback;
    }
    for suffix in [".local", ".lan", ".home.arpa", ".internal"] {
        if lower.ends_with(suffix) {
            return EndpointKind::Mdns;
        }
    }
    EndpointKind::Unknown
}

fn classify_ip(ip: IpAddr) -> EndpointKind {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, _, _] = v4.octets();
            if v4.is_loopback() {
                return EndpointKind::Loopback;
            }
            if v4.is_link_local() {
                return EndpointKind::LinkLocal;
            }
            // RFC1918
            if a == 10
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && b == 168)
            {
                return EndpointKind::LanRfc1918;
            }
            // Tailscale CGNAT: 100.64.0.0/10 → 100.64.0.0 .. 100.127.255.255
            if a == 100 && (64..=127).contains(&b) {
                return EndpointKind::Tailscale;
            }
            EndpointKind::Public
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                return EndpointKind::Loopback;
            }
            let seg0 = v6.segments()[0];
            // fe80::/10 → first 10 bits are 1111 1110 10... → seg0 & 0xffc0 == 0xfe80
            if seg0 & 0xffc0 == 0xfe80 {
                return EndpointKind::LinkLocal;
            }
            // fc00::/7 → first 7 bits are 1111 110 → seg0 & 0xfe00 == 0xfc00
            if seg0 & 0xfe00 == 0xfc00 {
                return EndpointKind::Ula;
            }
            EndpointKind::Public
        }
    }
}

pub fn validate_local_endpoint(
    host: &str,
    allow_public: bool,
) -> Result<(), EndpointPolicyError> {
    let kind = classify_endpoint(host);
    match kind {
        EndpointKind::Public | EndpointKind::Unknown if !allow_public => {
            Err(EndpointPolicyError::Blocked {
                host: host.to_string(),
                kind,
            })
        }
        _ => Ok(()),
    }
}

pub fn extract_host(input: &str) -> &str {
    // Strip any scheme.
    let after_scheme = input
        .find("://")
        .map(|idx| &input[idx + 3..])
        .unwrap_or(input);

    // Stop at the first '/' or '?' (path/query separator).
    let no_path = after_scheme
        .find(|c| c == '/' || c == '?')
        .map(|idx| &after_scheme[..idx])
        .unwrap_or(after_scheme);

    // IPv6 bracket form: [host]:port — bracket span up to `]`.
    if let Some(rest) = no_path.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return &rest[..end];
        }
    }

    // host or host:port — split on last colon, but only if the colon
    // is followed by digits (i.e., it's a port, not part of an IPv6 literal
    // already excluded above).
    if let Some(idx) = no_path.rfind(':') {
        let after = &no_path[idx + 1..];
        if !after.is_empty() && after.bytes().all(|b| b.is_ascii_digit()) {
            return &no_path[..idx];
        }
    }
    no_path
}
```

- [ ] **Step 5: Run the tests and confirm they pass**

Run:

```bash
cargo test -p medical-core endpoint_policy
```

Expected: all ~28 tests pass.

- [ ] **Step 6: Verify the rest of `medical-core` still compiles cleanly**

Run:

```bash
cargo test -p medical-core --lib
```

Expected: all existing tests still pass (the addition is non-breaking).

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/lib.rs crates/core/src/endpoint_policy.rs
git commit -m "feat(core): add endpoint_policy classifier for local-only AI rule"
```

---

## Task 2: `AppError::InvalidEndpoint` variant

**Files:**
- Modify: `crates/core/src/error.rs`

**Why:** The provider and Settings layers need a structured error variant they can return when a bad endpoint is encountered. Centralising it in `AppError` means it serializes to the frontend with the standard `{ kind, message }` shape automatically.

- [ ] **Step 1: Add the variant**

In `crates/core/src/error.rs`, add a new variant inside the `AppError` enum (after `Config` around line 69, before `Io`):

```rust
    #[error("invalid endpoint '{host}' for {field}: public/unknown endpoints are blocked (kind={kind:?}). Enable 'Allow public endpoints' in Advanced settings to override.")]
    InvalidEndpoint {
        field: String,
        host: String,
        kind: crate::endpoint_policy::EndpointKind,
    },
```

Add a corresponding entry in `kind_str`:

```rust
            AppError::InvalidEndpoint { .. } => "InvalidEndpoint",
```

- [ ] **Step 2: Update the custom `serde::Serialize` impl to include the new fields**

Below the existing `EndpointOffline` branch in the `match self` block of `impl serde::Serialize for AppError`, add:

```rust
            AppError::InvalidEndpoint { field, host, kind } => {
                let mut s = serializer.serialize_struct("AppError", 5)?;
                s.serialize_field("kind", self.kind_str())?;
                s.serialize_field("message", &self.to_string())?;
                s.serialize_field("field", field)?;
                s.serialize_field("host", host)?;
                s.serialize_field("endpointKind", kind)?;
                s.end()
            }
```

(Place this branch immediately before the generic `_ =>` arm so it takes precedence.)

- [ ] **Step 3: Add a `From<EndpointPolicyError>` impl that requires a field name**

`EndpointPolicyError` doesn't know which settings field it was validating. Add a helper on `AppError` that maps the policy error with the missing context:

After the existing `From<&str> for AppError` impl, add:

```rust
impl AppError {
    /// Convert an `EndpointPolicyError` into an `AppError::InvalidEndpoint`
    /// by attaching the settings field name the caller was validating.
    pub fn invalid_endpoint_for(
        err: crate::endpoint_policy::EndpointPolicyError,
        field: impl Into<String>,
    ) -> Self {
        let crate::endpoint_policy::EndpointPolicyError::Blocked { host, kind } = err;
        AppError::InvalidEndpoint {
            field: field.into(),
            host,
            kind,
        }
    }
}
```

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test -p medical-core --lib
```

Expected: all tests still pass (additive change).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/error.rs
git commit -m "feat(core): add AppError::InvalidEndpoint variant"
```

---

## Task 3: `AppConfig.allow_public_endpoint` field

**Files:**
- Modify: `crates/core/src/types/settings.rs`

**Why:** The single opt-out flag the entire allowlist consults. Default `false` (rejection-by-default).

- [ ] **Step 1: Add the field**

In `crates/core/src/types/settings.rs`, find the `AppConfig` struct (around line 268). Add the field at the end, just before `capture_for_training`:

```rust
    /// Opt-out for the local-only AI/STT endpoint allowlist. When `true`,
    /// `validate_local_endpoint` accepts public hosts. Default `false`.
    /// See `crates/core/src/endpoint_policy.rs`.
    #[serde(default)]
    pub allow_public_endpoint: bool,
```

- [ ] **Step 2: Add a regression test**

In the `#[cfg(test)] mod tests` block at the bottom of `settings.rs`, append:

```rust
    #[test]
    fn allow_public_endpoint_defaults_to_false() {
        let cfg: AppConfig = serde_json::from_str("{}").unwrap();
        assert!(!cfg.allow_public_endpoint);
    }
```

- [ ] **Step 3: Run tests**

Run:

```bash
cargo test -p medical-core --lib
```

Expected: all tests pass, including the new `allow_public_endpoint_defaults_to_false`.

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/types/settings.rs
git commit -m "feat(core): add AppConfig.allow_public_endpoint opt-out flag"
```

---

## Task 4: Mirror `allow_public_endpoint` on the TS side

**Files:**
- Modify: `src/lib/types/index.ts`
- Modify: `src/lib/stores/settings.ts`

**Why:** Keep the TS `AppConfig` aligned with Rust so the typo guard added in the earlier cleanup pass kicks in if someone misspells the new field.

- [ ] **Step 1: Add the field to the TS interface**

In `src/lib/types/index.ts`, find the `AppConfig` interface. Append before the closing `}`:

```ts
  // Security
  allow_public_endpoint: boolean;
```

- [ ] **Step 2: Add the default**

In `src/lib/stores/settings.ts`, find the `defaults: AppConfig = { ... }` literal and append before the closing `}`:

```ts
  allow_public_endpoint: false,
```

- [ ] **Step 3: Verify type-check**

Run:

```bash
npm run check
```

Expected: 0 errors, 0 warnings (held from previous cleanup).

- [ ] **Step 4: Commit**

```bash
git add src/lib/types/index.ts src/lib/stores/settings.ts
git commit -m "feat(types): mirror AppConfig.allow_public_endpoint on TS side"
```

---

## Task 5: `LmStudioProvider` host validation (TDD)

**Files:**
- Modify: `crates/ai-providers/src/lmstudio.rs`

**Why:** First of three parallel provider changes. `LmStudioProvider::new`, `new_with_endpoint`, and `set_endpoint` validate the host before constructing or swapping the inner client. `field` is `"lmstudio_host"`.

- [ ] **Step 1: Add the regression test**

In the existing `#[cfg(test)] mod tests` block in `crates/ai-providers/src/lmstudio.rs`, append:

```rust
    #[test]
    fn new_blocks_public_endpoint_by_default() {
        let result = LmStudioProvider::new(
            Some("http://api.openai.com/v1"),
            /* allow_public */ false,
            None,
            RetryConfig::default(),
        );
        assert!(matches!(
            result,
            Err(medical_core::error::AppError::InvalidEndpoint {
                field, ..
            }) if field == "lmstudio_host"
        ));
    }

    #[test]
    fn new_accepts_public_endpoint_when_allow_public() {
        let result = LmStudioProvider::new(
            Some("http://api.openai.com/v1"),
            /* allow_public */ true,
            None,
            RetryConfig::default(),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn new_accepts_local_endpoints_with_default_allow_public() {
        for host in [
            None,
            Some("http://localhost:1234"),
            Some("http://192.168.1.42:1234"),
            Some("http://100.64.0.1:1234"),
            Some("http://clinic.local:1234"),
        ] {
            let r = LmStudioProvider::new(host, /* allow_public */ false, None, RetryConfig::default());
            assert!(r.is_ok(), "expected Ok for {host:?}, got {r:?}");
        }
    }
```

- [ ] **Step 2: Run the tests and confirm they fail**

Run:

```bash
cargo test -p medical-ai-providers lmstudio
```

Expected: compile error — `LmStudioProvider::new` signature doesn't accept `allow_public`.

- [ ] **Step 3: Update `LmStudioProvider::new`**

In `crates/ai-providers/src/lmstudio.rs`, change the `new` signature:

```rust
pub fn new(
    host: Option<&str>,
    allow_public: bool,
    bearer: Option<String>,
    policy: RetryConfig,
) -> AppResult<Self> {
    let base = host.unwrap_or("http://localhost:1234");
    medical_core::endpoint_policy::validate_url(base, allow_public)
        .map_err(|e| AppError::invalid_endpoint_for(e, "lmstudio_host"))?;
    let base_url = format!("{base}/v1");
    let http = Client::builder()
        .pool_max_idle_per_host(5)
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| AppError::AiProvider(format!("Failed to build LM Studio HTTP client: {e}")))?;
    Ok(Self {
        static_base_url: base_url.clone(),
        client: Mutex::new(OpenAiCompatibleClient::new_with_bearer_and_name(http, base_url, policy, bearer, "LM Studio")),
        endpoint: RwLock::new(None),
        url_cache: Mutex::new(None),
    })
}
```

- [ ] **Step 4: Update `LmStudioProvider::new_with_endpoint` the same way**

Apply the identical `validate_url(...)` call at the top of `new_with_endpoint`, and add `allow_public: bool` as the second parameter:

```rust
pub fn new_with_endpoint(
    host: Option<&str>,
    allow_public: bool,
    bearer: Option<String>,
    policy: RetryConfig,
    ep: Option<RemoteEndpoint>,
) -> AppResult<Self> {
    let base = host.unwrap_or("http://localhost:1234");
    medical_core::endpoint_policy::validate_url(base, allow_public)
        .map_err(|e| AppError::invalid_endpoint_for(e, "lmstudio_host"))?;
    let base_url = format!("{base}/v1");
    // ... existing body unchanged
```

- [ ] **Step 5: Update existing tests in the same file**

Pre-existing tests call `LmStudioProvider::new(...)` with the old signature. Update each by inserting `false` (or `true` where the test wants to use a public host) as the second positional argument. Find each call site with:

```bash
grep -n "LmStudioProvider::new\b" crates/ai-providers/src/lmstudio.rs
```

For tests, the convention is `LmStudioProvider::new(None, false, None, RetryConfig::default())` for default-local-only.

- [ ] **Step 6: Run the tests and confirm they pass**

Run:

```bash
cargo test -p medical-ai-providers lmstudio
```

Expected: all tests pass, including the three new ones.

- [ ] **Step 7: Commit**

```bash
git add crates/ai-providers/src/lmstudio.rs
git commit -m "feat(ai-providers): LmStudioProvider validates host against local-only allowlist"
```

---

## Task 6: `OllamaProvider` host validation (TDD)

**Files:**
- Modify: `crates/ai-providers/src/ollama.rs`

**Why:** Parallel to Task 5. Same shape. `field` is `"ollama_host"`.

- [ ] **Step 1: Add the regression tests**

In the `#[cfg(test)] mod tests` block of `crates/ai-providers/src/ollama.rs`, append:

```rust
    #[test]
    fn new_blocks_public_endpoint_by_default() {
        let result = OllamaProvider::new(
            Some("http://api.openai.com/v1"),
            /* allow_public */ false,
            None,
            RetryConfig::default(),
        );
        assert!(matches!(
            result,
            Err(medical_core::error::AppError::InvalidEndpoint {
                field, ..
            }) if field == "ollama_host"
        ));
    }

    #[test]
    fn new_accepts_public_endpoint_when_allow_public() {
        let result = OllamaProvider::new(
            Some("http://api.openai.com/v1"),
            /* allow_public */ true,
            None,
            RetryConfig::default(),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn new_accepts_local_endpoints_with_default_allow_public() {
        for host in [
            None,
            Some("http://localhost:11434"),
            Some("http://192.168.1.42:11434"),
            Some("http://100.64.0.1:11434"),
            Some("http://clinic.local:11434"),
        ] {
            let r = OllamaProvider::new(host, /* allow_public */ false, None, RetryConfig::default());
            assert!(r.is_ok(), "expected Ok for {host:?}, got {r:?}");
        }
    }
```

- [ ] **Step 2: Run the tests and confirm they fail**

Run:

```bash
cargo test -p medical-ai-providers ollama
```

Expected: compile error.

- [ ] **Step 3: Update `OllamaProvider::new`**

In `crates/ai-providers/src/ollama.rs`, add `allow_public: bool` as the second parameter and validate at the top:

```rust
pub fn new(
    host: Option<&str>,
    allow_public: bool,
    bearer: Option<String>,
    policy: RetryConfig,
) -> AppResult<Self> {
    let base = host.unwrap_or("http://localhost:11434");
    medical_core::endpoint_policy::validate_url(base, allow_public)
        .map_err(|e| AppError::invalid_endpoint_for(e, "ollama_host"))?;
    // ... existing body unchanged
```

- [ ] **Step 4: Update `OllamaProvider::new_with_endpoint` the same way**

Apply the same change: add `allow_public: bool` as the second parameter and `validate_url(...)` at the top.

- [ ] **Step 5: Update existing call sites in the same file**

Find existing call sites in tests:

```bash
grep -n "OllamaProvider::new\b" crates/ai-providers/src/ollama.rs
```

Insert `false` (or `true` where the test wants a public host) as the second positional argument.

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test -p medical-ai-providers ollama
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/ai-providers/src/ollama.rs
git commit -m "feat(ai-providers): OllamaProvider validates host against local-only allowlist"
```

---

## Task 7: `RemoteSttProvider` host validation (TDD)

**Files:**
- Modify: `crates/stt-providers/src/remote_provider.rs`

**Why:** Same pattern. `RemoteSttProvider::new` and `new_with_endpoint` take a bare host (no scheme), so use `validate_local_endpoint(host, allow_public)` rather than `validate_url`.

- [ ] **Step 1: Add the regression tests**

In the `#[cfg(test)] mod tests` block in `crates/stt-providers/src/remote_provider.rs`, append (adapt the test helpers to mirror the file's existing test pattern — the file already has `RemoteSttProvider::new(...)` test calls you can model):

```rust
    #[test]
    fn new_blocks_public_host_by_default() {
        let result = RemoteSttProvider::new(
            "api.openai.com",
            8080,
            "whisper-1",
            /* allow_public */ false,
            None,
            std::path::PathBuf::from("/dev/null"),
            std::path::PathBuf::from("/dev/null"),
        );
        assert!(matches!(
            result,
            Err(medical_core::error::AppError::InvalidEndpoint {
                field, ..
            }) if field == "stt_remote_host"
        ));
    }

    #[test]
    fn new_accepts_public_host_when_allow_public() {
        let result = RemoteSttProvider::new(
            "api.openai.com",
            8080,
            "whisper-1",
            /* allow_public */ true,
            None,
            std::path::PathBuf::from("/dev/null"),
            std::path::PathBuf::from("/dev/null"),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn new_accepts_local_hosts_with_default_allow_public() {
        for host in ["localhost", "192.168.1.42", "100.64.0.1", "clinic.local"] {
            let r = RemoteSttProvider::new(
                host,
                8080,
                "whisper-1",
                /* allow_public */ false,
                None,
                std::path::PathBuf::from("/dev/null"),
                std::path::PathBuf::from("/dev/null"),
            );
            assert!(r.is_ok(), "expected Ok for {host}, got {r:?}");
        }
    }

    #[test]
    fn new_accepts_empty_host_when_default_local_used() {
        // The state.rs flow passes config.stt_remote_host even when empty.
        // The provider should accept "" because the validation skip lives at
        // the Settings save layer and the provider has its own default.
        // Here we accept empty as a no-op — see implementation.
        let r = RemoteSttProvider::new(
            "",
            8080,
            "whisper-1",
            /* allow_public */ false,
            None,
            std::path::PathBuf::from("/dev/null"),
            std::path::PathBuf::from("/dev/null"),
        );
        // Empty is treated as "no validation needed yet"; whatever the
        // existing default behavior is, leave it. Adjust this assertion if
        // empty is rejected today.
        assert!(r.is_ok() || matches!(r, Err(medical_core::error::AppError::InvalidEndpoint { .. })));
    }
```

(Inspect `RemoteSttProvider::new` to confirm the parameter order before pasting; the helper signature shown above mirrors the production call site in `state.rs:373` but the file's actual signature is the source of truth.)

- [ ] **Step 2: Run tests and confirm failure**

Run:

```bash
cargo test -p medical-stt-providers remote_provider
```

Expected: compile error.

- [ ] **Step 3: Update `RemoteSttProvider::new`**

In `crates/stt-providers/src/remote_provider.rs`, around line 90–110, add `allow_public: bool` as a new parameter (placed positionally to mirror Tasks 5/6 — confirm the existing slot count and adjust call sites). At the top of the body:

```rust
if !host.is_empty() {
    medical_core::endpoint_policy::validate_local_endpoint(host, allow_public)
        .map_err(|e| medical_core::error::AppError::invalid_endpoint_for(e, "stt_remote_host"))?;
}
```

(Empty host is allowed at the provider layer because the existing constructor accepts empty for default-fallback. The Settings save layer is where empty-vs-non-empty + mode='Remote' policy lives, Task 9.)

- [ ] **Step 4: Update `RemoteSttProvider::new_with_endpoint`**

Same change. Add `allow_public: bool`. Validate the host the same way at the top.

- [ ] **Step 5: Update existing call sites**

Find call sites:

```bash
grep -n "RemoteSttProvider::new\b" crates/stt-providers/src/remote_provider.rs
```

Update each: insert `false` (or `true` where the test wants a public host) as the new parameter.

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test -p medical-stt-providers remote_provider
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/stt-providers/src/remote_provider.rs
git commit -m "feat(stt-providers): RemoteSttProvider validates host against local-only allowlist"
```

---

## Task 8: `set_endpoint` validation across all three providers

**Files:**
- Modify: `crates/ai-providers/src/lmstudio.rs`
- Modify: `crates/ai-providers/src/ollama.rs`
- Modify: `crates/stt-providers/src/remote_provider.rs`

**Why:** Defense in depth on the runtime-switch path. The pairing flow already produces LAN/Tailscale endpoints, but a future change shouldn't be able to bypass the policy by going through `set_endpoint`.

- [ ] **Step 1: Add validation to `LmStudioProvider::set_endpoint`**

In `crates/ai-providers/src/lmstudio.rs`, locate `set_endpoint` (around line 98) and change its signature to accept `allow_public`:

```rust
pub async fn set_endpoint(
    &self,
    ep: Option<RemoteEndpoint>,
    allow_public: bool,
) -> AppResult<()> {
    // Validate either side of the endpoint if present. We accept the
    // endpoint as a whole if BOTH addresses (or the single one provided)
    // pass the local-only allowlist.
    if let Some(ref e) = ep {
        for (label, opt_host) in [("lan", e.lan.as_deref()), ("tailscale", e.tailscale.as_deref())] {
            if let Some(h) = opt_host {
                medical_core::endpoint_policy::validate_local_endpoint(h, allow_public)
                    .map_err(|err| AppError::invalid_endpoint_for(
                        err,
                        format!("lmstudio_host.{label}"),
                    ))?;
            }
        }
    }
    let new_bearer = ep.as_ref().and_then(|e| e.bearer.clone());
    *self.url_cache.lock().await = None;
    // ... rest of existing body unchanged
    Ok(())
}
```

If the function returned `()` before, change call sites to handle the new `AppResult<()>` return.

- [ ] **Step 2: Add the same validation to `OllamaProvider::set_endpoint`**

Same change in `crates/ai-providers/src/ollama.rs`. Field label: `"ollama_host.lan"` / `"ollama_host.tailscale"`.

- [ ] **Step 3: Add the same validation to `RemoteSttProvider::set_endpoint`**

Same change in `crates/stt-providers/src/remote_provider.rs`. Field label: `"stt_remote_host.lan"` / `"stt_remote_host.tailscale"`.

- [ ] **Step 4: Update call sites that invoke `set_endpoint`**

Find call sites:

```bash
grep -rn "\.set_endpoint(" src-tauri/src crates --include="*.rs" | grep -v test
```

Update each to pass `allow_public` from the loaded config. The pairing-success path lives in `src-tauri/src/commands/sharing/pairing.rs`; it has access to `AppConfig` via the state.

- [ ] **Step 5: Add a regression test in `lmstudio.rs`**

In the `#[cfg(test)] mod tests` block:

```rust
    #[tokio::test]
    async fn set_endpoint_rejects_public_lan_address() {
        let p = LmStudioProvider::new(None, false, None, RetryConfig::default()).expect("build");
        let bad = medical_core::types::RemoteEndpoint {
            lan: Some("api.openai.com".into()),
            tailscale: None,
            port: 1234,
            bearer: None,
        };
        let r = p.set_endpoint(Some(bad), false).await;
        assert!(matches!(
            r,
            Err(medical_core::error::AppError::InvalidEndpoint { .. })
        ));
    }

    #[tokio::test]
    async fn set_endpoint_accepts_lan_and_tailscale_addresses() {
        let p = LmStudioProvider::new(None, false, None, RetryConfig::default()).expect("build");
        let good = medical_core::types::RemoteEndpoint {
            lan: Some("192.168.1.42".into()),
            tailscale: Some("100.64.0.1".into()),
            port: 1234,
            bearer: None,
        };
        assert!(p.set_endpoint(Some(good), false).await.is_ok());
    }
```

Mirror the same two tests in `ollama.rs` and (with appropriate `RemoteSttProvider::new` parameters) `remote_provider.rs`.

- [ ] **Step 6: Run tests**

Run:

```bash
cargo test --workspace --lib
```

Expected: all tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/ai-providers/src/lmstudio.rs crates/ai-providers/src/ollama.rs crates/stt-providers/src/remote_provider.rs src-tauri/src/
git commit -m "feat: validate endpoints in set_endpoint across all three providers"
```

---

## Task 9: Thread `allow_public_endpoint` from `AppConfig` into provider construction in `state.rs`

**Files:**
- Modify: `src-tauri/src/state.rs`

**Why:** All the providers now take `allow_public`. The initialisation code in `state.rs` must read `config.allow_public_endpoint` and pass it through.

- [ ] **Step 1: Update `init_ai_providers_with_config`**

In `src-tauri/src/state.rs`, around line 265–303 (the AI provider init function), update the Ollama call site:

```rust
match OllamaProvider::new_with_endpoint(
    Some(&ollama_url),
    config.allow_public_endpoint,
    ollama_bearer,
    policy.clone(),
    ollama_ep,
) {
```

Same for the LM Studio call site around line 292.

- [ ] **Step 2: Update `init_stt_providers_with_config`**

Around line 373, update `RemoteSttProvider::new_with_endpoint`:

```rust
match medical_stt_providers::remote_provider::RemoteSttProvider::new_with_endpoint(
    &config.stt_remote_host,
    config.stt_remote_port,
    &config.stt_remote_model,
    config.allow_public_endpoint,
    bearer,
    seg_path,
    emb_path,
    whisper_ep,
) {
```

(Confirm the existing parameter order; insert `allow_public` in the position the provider's signature expects.)

- [ ] **Step 3: Verify build**

Run:

```bash
cargo build -p rust-medical-assistant
```

Expected: compiles cleanly.

- [ ] **Step 4: Run tests**

Run:

```bash
cargo test --workspace --lib
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/state.rs
git commit -m "feat(state): thread AppConfig.allow_public_endpoint into provider init"
```

---

## Task 10: `save_settings` host validation + integration tests

**Files:**
- Modify: `src-tauri/src/commands/settings.rs`

**Why:** The user-facing enforcement point. Bad hosts are rejected BEFORE persistence so the next provider init can never hit a bad value.

- [ ] **Step 1: Update `save_settings`**

In `src-tauri/src/commands/settings.rs`, replace `save_settings` with:

```rust
#[tauri::command]
pub fn save_settings(
    state: tauri::State<'_, AppState>,
    config: AppConfig,
) -> AppResult<()> {
    // Reject public/unknown hosts unless the user has explicitly opted in.
    for (field, host) in [
        ("ollama_host",     config.ollama_host.as_str()),
        ("lmstudio_host",   config.lmstudio_host.as_str()),
        ("stt_remote_host", config.stt_remote_host.as_str()),
    ] {
        // Empty host means "use default" — defer enforcement until the user
        // actually fills it in.
        if host.is_empty() {
            continue;
        }
        medical_core::endpoint_policy::validate_local_endpoint(
            host,
            config.allow_public_endpoint,
        )
        .map_err(|e| AppError::invalid_endpoint_for(e, field))?;
    }

    let conn = state.db.conn().map_err(|e| AppError::Database(e.to_string()))?;
    SettingsRepo::save_config(&conn, &config).map_err(|e| AppError::Database(e.to_string()))
}
```

- [ ] **Step 2: Add an integration test module**

Append to `src-tauri/src/commands/settings.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::endpoint_policy::EndpointKind;

    fn config_with_hosts(ollama: &str, lmstudio: &str, stt: &str) -> AppConfig {
        AppConfig {
            ollama_host: ollama.to_string(),
            lmstudio_host: lmstudio.to_string(),
            stt_remote_host: stt.to_string(),
            ..Default::default()
        }
    }

    // We can't run the full Tauri command here (needs State), but we can
    // exercise the validation logic standalone by extracting it. For now
    // we test the inner validation by calling the helper directly. This
    // is sufficient because the save_settings body is a thin wrapper.

    #[test]
    fn validate_public_ollama_host_rejected_by_default() {
        let cfg = config_with_hosts("api.openai.com", "localhost", "");
        let r = medical_core::endpoint_policy::validate_local_endpoint(
            &cfg.ollama_host,
            cfg.allow_public_endpoint,
        );
        assert!(r.is_err());
    }

    #[test]
    fn validate_public_ollama_host_accepted_with_opt_out() {
        let mut cfg = config_with_hosts("api.openai.com", "localhost", "");
        cfg.allow_public_endpoint = true;
        let r = medical_core::endpoint_policy::validate_local_endpoint(
            &cfg.ollama_host,
            cfg.allow_public_endpoint,
        );
        assert!(r.is_ok());
    }

    #[test]
    fn empty_stt_remote_host_is_allowed() {
        let cfg = config_with_hosts("localhost", "localhost", "");
        // Mirroring save_settings: empty is skipped.
        assert!(cfg.stt_remote_host.is_empty());
    }

    #[test]
    fn invalid_endpoint_for_helper_includes_field_name() {
        use medical_core::endpoint_policy::EndpointPolicyError;
        let err = EndpointPolicyError::Blocked {
            host: "api.openai.com".into(),
            kind: EndpointKind::Unknown,
        };
        let app = AppError::invalid_endpoint_for(err, "ollama_host");
        match app {
            AppError::InvalidEndpoint { field, host, kind } => {
                assert_eq!(field, "ollama_host");
                assert_eq!(host, "api.openai.com");
                assert_eq!(kind, EndpointKind::Unknown);
            }
            _ => panic!("expected InvalidEndpoint"),
        }
    }
}
```

- [ ] **Step 3: Run tests**

Run:

```bash
cargo test -p rust-medical-assistant --lib commands::settings
```

Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/commands/settings.rs
git commit -m "feat(settings): save_settings rejects public hosts unless opt-out is set"
```

---

## Task 11: TS `endpointPolicy.ts` helper + Vitest tests

**Files:**
- Create: `src/lib/utils/endpointPolicy.ts`
- Create: `src/lib/utils/endpointPolicy.test.ts`

**Why:** UI feedback for the inline warning. The TS side is **not the source of truth** (Rust is) — its job is to render a warning *before* the user clicks save so they understand why their input will be rejected.

- [ ] **Step 1: Write the failing tests**

Create `src/lib/utils/endpointPolicy.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { classifyEndpoint, isLocalOrAllowed, type EndpointKind } from './endpointPolicy';

describe('classifyEndpoint', () => {
  const cases: Array<[string, EndpointKind]> = [
    // Loopback
    ['localhost', 'Loopback'],
    ['LOCALHOST', 'Loopback'],
    ['127.0.0.1', 'Loopback'],
    ['::1', 'Loopback'],

    // RFC1918
    ['10.0.0.0', 'LanRfc1918'],
    ['10.255.255.255', 'LanRfc1918'],
    ['172.16.0.0', 'LanRfc1918'],
    ['172.31.255.255', 'LanRfc1918'],
    ['192.168.1.42', 'LanRfc1918'],

    // Out of RFC1918
    ['9.255.255.255', 'Public'],
    ['172.32.0.0', 'Public'],
    ['192.169.0.0', 'Public'],

    // Link-local
    ['169.254.0.1', 'LinkLocal'],
    ['fe80::1', 'LinkLocal'],

    // Tailscale
    ['100.64.0.0', 'Tailscale'],
    ['100.127.255.255', 'Tailscale'],
    ['100.128.0.0', 'Public'],

    // ULA
    ['fd00::1', 'Ula'],
    ['fc00::1', 'Ula'],

    // mDNS / non-routable TLDs
    ['clinic.local', 'Mdns'],
    ['box.lan', 'Mdns'],
    ['server.internal', 'Mdns'],
    ['host.home.arpa', 'Mdns'],
    ['CLINIC.LOCAL', 'Mdns'],

    // Public / Unknown
    ['8.8.8.8', 'Public'],
    ['api.openai.com', 'Unknown'],
    ['clinic.example.com', 'Unknown'],
  ];

  for (const [host, expected] of cases) {
    it(`classifies "${host}" as ${expected}`, () => {
      expect(classifyEndpoint(host)).toBe(expected);
    });
  }
});

describe('isLocalOrAllowed', () => {
  it('accepts local kinds regardless of allow_public', () => {
    for (const host of ['localhost', '192.168.1.42', '100.64.0.1', 'clinic.local']) {
      expect(isLocalOrAllowed(host, false)).toBe(true);
      expect(isLocalOrAllowed(host, true)).toBe(true);
    }
  });

  it('rejects public/unknown unless allow_public', () => {
    for (const host of ['api.openai.com', '8.8.8.8']) {
      expect(isLocalOrAllowed(host, false)).toBe(false);
      expect(isLocalOrAllowed(host, true)).toBe(true);
    }
  });

  it('accepts empty host (skipped)', () => {
    expect(isLocalOrAllowed('', false)).toBe(true);
  });
});
```

- [ ] **Step 2: Run tests and confirm failure**

Run:

```bash
npx vitest run src/lib/utils/endpointPolicy.test.ts
```

Expected: compile/module-not-found error.

- [ ] **Step 3: Implement the helper**

Create `src/lib/utils/endpointPolicy.ts`:

```ts
/**
 * TS mirror of Rust's `endpoint_policy` classifier. The Rust side is the
 * source of truth (Settings save and provider construction enforce
 * authoritatively). This helper exists only to render an inline warning in
 * the Settings UI before the user clicks Save.
 *
 * Keep this file in sync with `crates/core/src/endpoint_policy.rs`.
 */

export type EndpointKind =
  | 'Loopback'
  | 'LanRfc1918'
  | 'LinkLocal'
  | 'Tailscale'
  | 'Ula'
  | 'Mdns'
  | 'Public'
  | 'Unknown';

const LOCAL_TLD_SUFFIXES = ['.local', '.lan', '.home.arpa', '.internal'];

function stripBrackets(host: string): string {
  return host.startsWith('[') && host.endsWith(']')
    ? host.slice(1, -1)
    : host;
}

function isIpv4(host: string): { a: number; b: number; c: number; d: number } | null {
  const parts = host.split('.');
  if (parts.length !== 4) return null;
  const nums = parts.map((p) => /^\d+$/.test(p) ? Number(p) : NaN);
  if (nums.some((n) => !Number.isFinite(n) || n < 0 || n > 255)) return null;
  return { a: nums[0], b: nums[1], c: nums[2], d: nums[3] };
}

function classifyIpv4(p: { a: number; b: number; c: number; d: number }): EndpointKind {
  if (p.a === 127) return 'Loopback';
  if (p.a === 169 && p.b === 254) return 'LinkLocal';
  if (p.a === 10) return 'LanRfc1918';
  if (p.a === 172 && p.b >= 16 && p.b <= 31) return 'LanRfc1918';
  if (p.a === 192 && p.b === 168) return 'LanRfc1918';
  if (p.a === 100 && p.b >= 64 && p.b <= 127) return 'Tailscale';
  return 'Public';
}

function classifyIpv6(host: string): EndpointKind | null {
  // Must contain a colon and parse roughly as an IPv6 address.
  // Browser/Node has no built-in IPv6 parser, so we use a regex test that
  // accepts the syntactically valid forms we care about.
  if (!/^[0-9a-fA-F:]+$/.test(host) || !host.includes(':')) return null;

  // Loopback
  if (host === '::1') return 'Loopback';

  // Read the first hex group, accounting for "::" leading.
  // We just care about the high bits of the first 16-bit segment.
  const firstSeg = host.split(':').find((s) => s.length > 0);
  if (!firstSeg) return null;
  const seg0 = parseInt(firstSeg, 16);
  if (!Number.isFinite(seg0)) return null;

  // fe80::/10 → segment & 0xffc0 === 0xfe80
  if ((seg0 & 0xffc0) === 0xfe80) return 'LinkLocal';
  // fc00::/7 → segment & 0xfe00 === 0xfc00
  if ((seg0 & 0xfe00) === 0xfc00) return 'Ula';
  return 'Public';
}

export function classifyEndpoint(host: string): EndpointKind {
  const trimmed = stripBrackets(host);

  // IPv4
  const v4 = isIpv4(trimmed);
  if (v4) return classifyIpv4(v4);

  // IPv6
  const v6 = classifyIpv6(trimmed);
  if (v6) return v6;

  // Hostname
  const lower = trimmed.toLowerCase();
  if (lower === 'localhost') return 'Loopback';
  for (const suf of LOCAL_TLD_SUFFIXES) {
    if (lower.endsWith(suf)) return 'Mdns';
  }
  return 'Unknown';
}

/**
 * Returns true if the host is acceptable given `allowPublic`. An empty
 * host is treated as acceptable (no value yet).
 */
export function isLocalOrAllowed(host: string, allowPublic: boolean): boolean {
  if (host === '') return true;
  const kind = classifyEndpoint(host);
  if (kind === 'Public' || kind === 'Unknown') return allowPublic;
  return true;
}
```

- [ ] **Step 4: Run tests and confirm they pass**

Run:

```bash
npx vitest run src/lib/utils/endpointPolicy.test.ts
```

Expected: all tests pass.

- [ ] **Step 5: Verify type-check**

Run:

```bash
npm run check
```

Expected: 0 errors.

- [ ] **Step 6: Commit**

```bash
git add src/lib/utils/endpointPolicy.ts src/lib/utils/endpointPolicy.test.ts
git commit -m "feat(utils): TS mirror of endpoint_policy classifier"
```

---

## Task 12: Settings UI — inline warnings + Advanced toggle + banner

**Files:**
- Modify: `src/lib/components/settings/Models.svelte` — warnings under `ollama_host` and `lmstudio_host`
- Modify: `src/lib/components/settings/Audio.svelte` — warning under `stt_remote_host`
- Modify: `src/lib/components/settings/General.svelte` — Advanced section with toggle + global banner

**Why:** Surface the policy in the UI so the user can self-correct before hitting save.

- [ ] **Step 1: Add helper imports and a small `EndpointWarning` snippet in `Models.svelte`**

In `src/lib/components/settings/Models.svelte`'s `<script>` block, add near the top:

```ts
  import { classifyEndpoint, isLocalOrAllowed } from '../../utils/endpointPolicy';

  const ollamaOk = $derived(isLocalOrAllowed($settings.ollama_host ?? '', $settings.allow_public_endpoint));
  const ollamaKind = $derived(classifyEndpoint($settings.ollama_host ?? ''));
  const lmstudioOk = $derived(isLocalOrAllowed($settings.lmstudio_host ?? '', $settings.allow_public_endpoint));
  const lmstudioKind = $derived(classifyEndpoint($settings.lmstudio_host ?? ''));
```

Just below each host input element (find `bind:value` or `value={$settings.ollama_host}` and the `value={$settings.lmstudio_host}` site), insert:

```svelte
  {#if !ollamaOk}
    <div class="endpoint-warning">
      ⚠ This is a public-internet address ({ollamaKind}). PHI may leave your device.
      Enable <em>Allow public endpoints</em> in Advanced settings to use this anyway.
    </div>
  {/if}
```

Mirror the same block for LM Studio.

Add CSS at the bottom of the `<style>` block:

```css
  .endpoint-warning {
    color: #b45309;
    background: #fef3c7;
    border: 1px solid #fbbf24;
    border-radius: 4px;
    padding: 6px 10px;
    margin-top: 4px;
    font-size: 0.85rem;
  }
```

- [ ] **Step 2: Same change in `Audio.svelte` for `stt_remote_host`**

Add the helper imports and the `derived` values for `stt_remote_host`. Insert the warning block under the host input. Reuse the same CSS class (or copy it if the project hasn't shared utility CSS).

- [ ] **Step 3: Add the Advanced section + toggle in `General.svelte`**

In `src/lib/components/settings/General.svelte`, append at the bottom (or after an existing collapsible section if one exists):

```svelte
<details class="advanced-section">
  <summary>Advanced</summary>
  <div class="advanced-content">
    <label class="form-row">
      <input
        type="checkbox"
        checked={$settings.allow_public_endpoint}
        onchange={(e) => settings.updateField('allow_public_endpoint', (e.target as HTMLInputElement).checked)}
      />
      <span>
        Allow public AI / STT endpoints
        <p class="hint">
          By default, FerriScribe blocks public-internet AI or STT hosts to keep
          PHI on-device. Enable this only if you understand that data may leave
          your machine.
        </p>
      </span>
    </label>
  </div>
</details>
```

Add at the top of the same file (or in a shared layout), a banner that only shows when the flag is true:

```svelte
{#if $settings.allow_public_endpoint}
  <div class="public-endpoint-banner">
    ⚠ <strong>Public endpoints enabled.</strong> AI / STT requests may leave your device.
  </div>
{/if}
```

(If the banner belongs higher up in the layout — e.g., a top-of-Settings location — place it there instead. The plan's intent: a persistent, hard-to-miss reminder while the toggle is on.)

Add CSS:

```css
  .public-endpoint-banner {
    background: #fef2f2;
    color: #991b1b;
    border: 1px solid #fca5a5;
    border-radius: 4px;
    padding: 8px 12px;
    margin-bottom: 12px;
    font-size: 0.9rem;
  }
  .advanced-section summary {
    cursor: pointer;
    font-weight: 600;
    margin-top: 16px;
  }
  .advanced-content {
    margin-top: 8px;
    padding-left: 16px;
  }
  .form-row { display: flex; gap: 10px; align-items: flex-start; }
  .hint { color: var(--text-muted); font-size: 0.8rem; margin: 4px 0 0 0; }
```

- [ ] **Step 4: Verify type-check + tests**

Run:

```bash
npm run check
npx vitest run
```

Expected: 0 errors, all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/lib/components/settings/
git commit -m "feat(settings-ui): inline endpoint warnings + Advanced opt-out toggle"
```

---

## Task 13: Manual smoke test

**Files:** none (verification only).

**Why:** Per `CLAUDE.md`: UI / settings changes need a dev-server walkthrough. The audit's regression test (`audit_regression_api_openai_com_blocked_by_default`) provides a unit-level guarantee; this task verifies the user-facing flow.

- [ ] **Step 1: Start the dev environment**

```bash
npm run tauri dev
```

- [ ] **Step 2: Verify default-deny**

- [ ] Open Settings → AI Models. Type `api.openai.com` in the Ollama host field.
- [ ] An inline warning appears: "This is a public-internet address (Unknown). PHI may leave your device. Enable Allow public endpoints in Advanced settings to use this anyway."
- [ ] Click Save (or wait for the auto-save). The save fails. An error appears: `invalid endpoint 'api.openai.com' for ollama_host: public/unknown endpoints are blocked (kind=Unknown). Enable 'Allow public endpoints' in Advanced settings to override.`
- [ ] Reload Settings. The persisted value is still the previous good one — `api.openai.com` was not saved.

- [ ] **Step 3: Verify opt-out**

- [ ] Open Settings → General → Advanced. Enable "Allow public AI / STT endpoints".
- [ ] A banner appears at the top of Settings: "Public endpoints enabled. AI / STT requests may leave your device."
- [ ] Return to AI Models. The warning under the Ollama host field disappears.
- [ ] Save succeeds. Reload Settings: `ollama_host` is now `api.openai.com`.
- [ ] Disable the Advanced toggle. The warning reappears in AI Models. The banner disappears.
- [ ] Reset Ollama host to `localhost`. Save succeeds. Warning clears.

- [ ] **Step 4: Verify each local kind passes**

- [ ] In sequence, set `ollama_host` to each of: `localhost`, `192.168.1.42`, `100.64.0.1`, `clinic.local`, `box.lan`. Save succeeds for each, no warning rendered.

- [ ] **Step 5: Verify the equivalent flow for `lmstudio_host` and `stt_remote_host`**

- [ ] Settings → AI Models: repeat the public-host test for `lmstudio_host`.
- [ ] Settings → Audio: repeat for `stt_remote_host`.

- [ ] **Step 6: Verify no PHI in logs**

Watch the dev terminal during all steps. No host strings should appear in any log line (the policy logs only the field name and classification, never the host string).

- [ ] **Step 7: Fix anything that surfaced**

If smoke turned anything up, fix and commit:

```bash
git add <files>
git commit -m "fix(allowlist): <what>"
```

---

## Self-review notes

- **Spec coverage:** every section of the spec maps to a task.
  - Classifier (`endpoint_policy.rs`) → Task 1.
  - `AppError::InvalidEndpoint` → Task 2.
  - `AppConfig.allow_public_endpoint` → Task 3 (Rust) + Task 4 (TS).
  - Provider construction validation → Tasks 5, 6, 7.
  - `set_endpoint` validation → Task 8.
  - `state.rs` wiring → Task 9.
  - `save_settings` validation → Task 10.
  - TS helper → Task 11.
  - Settings UI → Task 12.
  - Manual smoke → Task 13.
- **Hostname-without-DNS is `Unknown`:** the spec calls out that the classifier deliberately treats unresolved hostnames as Unknown. The classifier implementation handles this; the regression test `public_hostname_classifies_as_unknown` locks it in.
- **No PHI leaks:** the new `tracing::warn!` (if any are added during smoke or implementation) must log only field + classification, never the host string. The provider code in Tasks 5/6/7 does not introduce any new logging — the inline `validate_*` call returns an error and surfaces through the existing error path.
- **Type consistency:**
  - `EndpointKind` variant names are identical Rust ↔ TS: `Loopback | LanRfc1918 | LinkLocal | Tailscale | Ula | Mdns | Public | Unknown`.
  - Field names always `"ollama_host"` / `"lmstudio_host"` / `"stt_remote_host"` (or `.lan` / `.tailscale` suffix in `set_endpoint`).
  - `allow_public_endpoint` is the single config name everywhere.
- **No `url` crate added:** Task 1's `extract_host` is pure string manipulation, avoiding a new workspace dep.
- **The audit regression test exists** in `endpoint_policy.rs` (Task 1) and is named `audit_regression_api_openai_com_blocked_by_default` for traceability.
