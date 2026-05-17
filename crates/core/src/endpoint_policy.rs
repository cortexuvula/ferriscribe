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

pub fn classify_endpoint(host: &str) -> EndpointKind {
    // Strip outer IPv6 brackets if present: "[fd00::1]" -> "fd00::1".
    let trimmed = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);

    // First try as an IP literal.
    if let Ok(ip) = trimmed.parse::<std::net::IpAddr>() {
        return classify_ip(ip);
    }

    // Otherwise it's a hostname. Case-insensitive checks.
    let lower = trimmed.to_ascii_lowercase();
    // Defensive: normalize away any trailing dot(s) so fully-qualified domain
    // names like "foo.ts.net." or "clinic.local." still match the suffix
    // checks below. Mirrors the `trim_end_matches('.')` idiom used by
    // `parse_self_dns_name` in `crates/sharing/src/tailscale.rs`.
    let lower = lower.trim_end_matches('.');
    if lower == "localhost" {
        return EndpointKind::Loopback;
    }
    // Tailscale MagicDNS: <machine>.<tailnet>.ts.net. Match the FQDN suffix
    // ".ts.net" (with leading dot) so we don't false-positive on things like
    // "fakets.net". This is a static, DNS-free trust signal.
    if lower.ends_with(".ts.net") {
        return EndpointKind::Tailscale;
    }
    for suffix in [".local", ".lan", ".home.arpa", ".internal"] {
        if lower.ends_with(suffix) {
            return EndpointKind::Mdns;
        }
    }
    EndpointKind::Unknown
}

fn classify_ip(ip: std::net::IpAddr) -> EndpointKind {
    match ip {
        std::net::IpAddr::V4(v4) => {
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
        std::net::IpAddr::V6(v6) => {
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

/// Extract the bare host from a string that may be a bare host, host:port,
/// or scheme://host:port/path. Returns the host with any surrounding IPv6
/// brackets stripped. Returns the original input if it can't be parsed.
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

    #[test]
    fn tailscale_magicdns_suffix() {
        // Real-world shape from the bug report.
        assert_eq!(
            classify_endpoint("mac.tail161478.ts.net"),
            EndpointKind::Tailscale
        );
        // Tailnet name with a dash is valid.
        assert_eq!(
            classify_endpoint("clinic.example-tailnet.ts.net"),
            EndpointKind::Tailscale
        );
        // Case-insensitive: classification lowercases the hostname first.
        assert_eq!(
            classify_endpoint("MAC.TAILNET.TS.NET"),
            EndpointKind::Tailscale
        );
        // Minimal MagicDNS-shaped host directly under .ts.net.
        assert_eq!(classify_endpoint("server.ts.net"), EndpointKind::Tailscale);
    }

    #[test]
    fn tailscale_magicdns_partial_match_is_unknown() {
        // The bare apex "ts.net" does NOT end with ".ts.net" (no leading dot),
        // so it should NOT be classified as Tailscale. Treat as Unknown.
        assert_eq!(classify_endpoint("ts.net"), EndpointKind::Unknown);
        // ".ts.net" appears mid-string, not as the suffix.
        assert_eq!(
            classify_endpoint("notreally.ts.net.example.com"),
            EndpointKind::Unknown
        );
        // Ends with "ts.net" but NOT ".ts.net" — must not false-match.
        assert_eq!(classify_endpoint("fakets.net"), EndpointKind::Unknown);
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

    // ── classify_endpoint: trailing-dot FQDN normalization ─────────
    #[test]
    fn tailscale_magicdns_with_trailing_dot() {
        // Fully-qualified DNS name with a trailing root-zone dot must still
        // classify as Tailscale.
        assert_eq!(
            classify_endpoint("mac.tail161478.ts.net."),
            EndpointKind::Tailscale
        );
        // Pathological double trailing dot — defensive guard.
        assert_eq!(
            classify_endpoint("foo.ts.net.."),
            EndpointKind::Tailscale
        );
    }

    #[test]
    fn mdns_with_trailing_dot() {
        assert_eq!(classify_endpoint("clinic.local."), EndpointKind::Mdns);
        assert_eq!(classify_endpoint("host.home.arpa."), EndpointKind::Mdns);
        assert_eq!(classify_endpoint("server.internal."), EndpointKind::Mdns);
    }

    #[test]
    fn localhost_with_trailing_dot() {
        assert_eq!(classify_endpoint("localhost."), EndpointKind::Loopback);
        assert_eq!(classify_endpoint("LOCALHOST."), EndpointKind::Loopback);
    }

    #[test]
    fn trailing_dot_does_not_break_unknown() {
        // Normalization must not turn public domains into something else.
        assert_eq!(classify_endpoint("api.openai.com."), EndpointKind::Unknown);
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
            "100.64.0.1",                  // Tailscale CGNAT
            "fd7a:115c:a1e0::1",           // Tailscale ULA
            "mac.tail161478.ts.net",       // Tailscale MagicDNS
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

    #[test]
    fn validate_accepts_tailscale_magicdns_without_allow_public() {
        // Regression: remote clients pairing via Tailscale MagicDNS were being
        // rejected because *.ts.net fell through to Unknown. Tailscale is a
        // trusted local-network kind, so this must succeed even when
        // allow_public_endpoint = false.
        assert!(validate_local_endpoint("mac.tail161478.ts.net", false).is_ok());
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
