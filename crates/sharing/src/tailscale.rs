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
pub async fn self_dns_name() -> Option<String> {
    let out = tokio::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_self_dns_name(&out.stdout)
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
