//! Parser for `tailscale status --json` output.
//!
//! Extracted into a pure function so we can fixture-test it without running
//! the Tailscale binary. Used by `src-tauri` to populate the Tailscale DNS
//! name in the QR payload, and by the orchestrator to advertise it in
//! `InfoSnapshot` and the mDNS TXT record so LAN-paired clients learn it.

use serde_json::Value;

/// Parse the machine's Tailscale DNS name from `tailscale status --json` output.
///
/// Returns `Self.DNSName` with the trailing dot stripped (Tailscale always
/// appends one). Returns `None` for malformed JSON, missing `Self`, or
/// missing `DNSName` fields.
///
/// # Example
///
/// ```
/// use medical_sharing::tailscale::parse_self_dns_name;
///
/// let json = br#"{"Self":{"DNSName":"clinic.tail-abc.ts.net."}}"#;
/// assert_eq!(parse_self_dns_name(json), Some("clinic.tail-abc.ts.net".into()));
/// ```
pub fn parse_self_dns_name(json: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(json).ok()?;
    let dns = v.get("Self")?.get("DNSName")?.as_str()?;
    Some(dns.trim_end_matches('.').to_string())
}

/// Discover this machine's own Tailscale DNS name by shelling out to the
/// `tailscale` CLI.
///
/// Wraps the pure [`parse_self_dns_name`] parser with the subprocess
/// invocation. Returns `None` when the Tailscale binary is absent, not
/// authenticated, or the machine has no Tailscale DNS name. The call is
/// best-effort: callers (the orchestrator's `rebuild_info_snapshot`, the
/// QR pairing path) treat `None` as "no Tailscale address available" and
/// degrade gracefully.
///
/// macOS note: GUI apps launched from Finder/Dock inherit a minimal PATH
/// (`/usr/bin:/bin:/usr/sbin:/sbin`) that does NOT include Homebrew's
/// `/opt/homebrew/bin` or `/usr/local/bin`. So in addition to relying on
/// PATH, we probe known installation locations as a fallback.
pub async fn self_dns_name() -> Option<String> {
    let output = run_tailscale_status_json().await?;
    parse_self_dns_name(&output)
}

/// Run `tailscale status --json`, searching PATH first, then common known
/// installation locations (Homebrew Apple Silicon + Intel, Linux/WSL).
/// Returns the raw stdout bytes on success.
///
/// `pub` so the Tauri command layer (e.g. `tailscale_peers` in discovery.rs)
/// can reuse the same location-fallback logic instead of duplicating it.
pub async fn run_tailscale_status_json() -> Option<Vec<u8>> {
    // Candidate binaries in order: bare name (PATH), then known absolute
    // paths for macOS GUI apps and Linux.
    let candidates: Vec<&str> = vec![
        "tailscale", // PATH lookup (works for CLI/terminal-launched apps)
        "/opt/homebrew/bin/tailscale", // macOS Homebrew (Apple Silicon)
        "/usr/local/bin/tailscale", // macOS Homebrew (Intel) + Mac App Store CLI
        "/usr/bin/tailscale", // Linux package manager
    ];
    for candidate in candidates {
        match tokio::process::Command::new(candidate)
            .args(["status", "--json"])
            .output()
            .await
        {
            Ok(out) if out.status.success() => {
                tracing::debug!(binary = candidate, "tailscale status succeeded");
                return Some(out.stdout);
            }
            Ok(out) => {
                tracing::debug!(
                    binary = candidate,
                    exit_code = out.status.code(),
                    "tailscale: non-zero exit, trying next candidate"
                );
            }
            Err(e) => {
                tracing::debug!(
                    binary = candidate,
                    kind = ?e.kind(),
                    "tailscale: spawn failed, trying next candidate"
                );
            }
        }
    }
    tracing::info!("tailscale: no candidate binary found (Tailscale not installed or not running)");
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_json_with_dnsname() {
        let json = br#"{"Self":{"DNSName":"clinic-laptop.tail-scale.ts.net."}}"#;
        assert_eq!(
            parse_self_dns_name(json),
            Some("clinic-laptop.tail-scale.ts.net".to_string()),
            "trailing dot should be stripped"
        );
    }

    #[test]
    fn parses_valid_json_without_trailing_dot() {
        let json = br#"{"Self":{"DNSName":"host.example"}}"#;
        assert_eq!(parse_self_dns_name(json), Some("host.example".to_string()));
    }

    #[test]
    fn returns_none_for_empty_input() {
        assert_eq!(parse_self_dns_name(b""), None);
    }

    #[test]
    fn returns_none_when_self_field_is_missing() {
        let json = br#"{"Other":{"DNSName":"x"}}"#;
        assert_eq!(parse_self_dns_name(json), None);
    }

    #[test]
    fn returns_none_when_dnsname_field_is_missing() {
        let json = br#"{"Self":{"OtherField":"x"}}"#;
        assert_eq!(parse_self_dns_name(json), None);
    }

    #[test]
    fn returns_none_for_malformed_json() {
        assert_eq!(parse_self_dns_name(b"not json at all"), None);
        assert_eq!(parse_self_dns_name(b"{\"Self\":"), None);
    }
}
