//! Parser for `tailscale status --json` output. Extracted into a pure
//! function so we can fixture-test it without running the binary.

use serde_json::Value;

/// Given the bytes of a `tailscale status --json` output, return the
/// `Self.DNSName` (with any trailing dot stripped) if present.
///
/// Returns `None` for malformed JSON, missing `Self`, or missing DNS
/// name.
pub fn parse_self_dns_name(json: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(json).ok()?;
    let dns = v.get("Self")?.get("DNSName")?.as_str()?;
    Some(dns.trim_end_matches('.').to_string())
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
