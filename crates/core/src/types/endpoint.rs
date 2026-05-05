//! `RemoteEndpoint` — LAN/Tailscale connection resolver with optional bearer auth.

/// Build the host:port portion of an HTTP URL, bracketing IPv6 literals as
/// required by RFC 3986. Without this, `http://fe80::1:8080` parses as host
/// `fe80` with three nested ports — reqwest then emits a "Builder error"
/// rather than a useful message. Hostnames and IPv4 literals pass through
/// unchanged.
pub fn http_url(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("http://[{host}]:{port}")
    } else {
        format!("http://{host}:{port}")
    }
}

/// A remote endpoint that may be reachable on either a LAN address or a
/// Tailscale address. The resolver probes LAN first with a short connect
/// timeout, then falls back to Tailscale.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RemoteEndpoint {
    /// LAN IP or hostname (no scheme, no port).
    pub lan: Option<String>,
    /// Tailscale IP or hostname (no scheme, no port).
    pub tailscale: Option<String>,
    /// TCP port to connect on.
    pub port: u16,
    /// Optional bearer token sent as `Authorization: Bearer <token>`.
    pub bearer: Option<String>,
}

/// Manual `Debug` impl that redacts the bearer token so it can never appear
/// in `tracing::debug!(?endpoint, …)` output or any other log sink.
impl std::fmt::Debug for RemoteEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteEndpoint")
            .field("lan", &self.lan)
            .field("tailscale", &self.tailscale)
            .field("port", &self.port)
            .field("bearer", &self.bearer.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl RemoteEndpoint {
    /// Probe the LAN address with a 500 ms connect timeout, fall back to
    /// Tailscale (2 s timeout). Returns the URL prefix (e.g.
    /// `"http://192.168.1.42:11435"`) for the first reachable address, or
    /// `None` if neither is reachable.
    pub async fn resolve_base_url(&self) -> Option<String> {
        if let Some(lan) = &self.lan {
            if Self::can_connect(lan, self.port, std::time::Duration::from_millis(500)).await {
                return Some(http_url(lan, self.port));
            }
        }
        if let Some(ts) = &self.tailscale {
            if Self::can_connect(ts, self.port, std::time::Duration::from_secs(2)).await {
                return Some(http_url(ts, self.port));
            }
        }
        None
    }

    async fn can_connect(host: &str, port: u16, timeout: std::time::Duration) -> bool {
        tokio::time::timeout(
            timeout,
            tokio::net::TcpStream::connect((host, port)),
        )
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_url_brackets_ipv6_literal() {
        assert_eq!(
            http_url("fd9f:e471:9b3d:99e2:841:fadf:c810:ea21", 11436),
            "http://[fd9f:e471:9b3d:99e2:841:fadf:c810:ea21]:11436"
        );
        assert_eq!(http_url("fe80::1", 8080), "http://[fe80::1]:8080");
    }

    #[test]
    fn http_url_passes_ipv4_and_hostname_through() {
        assert_eq!(http_url("192.168.1.42", 11434), "http://192.168.1.42:11434");
        assert_eq!(http_url("clinic.local", 11434), "http://clinic.local:11434");
    }

    #[test]
    fn http_url_does_not_double_bracket() {
        assert_eq!(http_url("[fe80::1]", 8080), "http://[fe80::1]:8080");
    }

    #[test]
    fn default_endpoint_has_no_fields() {
        let ep = RemoteEndpoint::default();
        assert!(ep.lan.is_none());
        assert!(ep.tailscale.is_none());
        assert_eq!(ep.port, 0);
        assert!(ep.bearer.is_none());
    }

    #[test]
    fn roundtrip_serde() {
        let ep = RemoteEndpoint {
            lan: Some("192.168.1.42".into()),
            tailscale: Some("100.64.0.1".into()),
            port: 11434,
            bearer: Some("tok_abc".into()),
        };
        let json = serde_json::to_string(&ep).expect("serialize");
        let back: RemoteEndpoint = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.lan.as_deref(), Some("192.168.1.42"));
        assert_eq!(back.tailscale.as_deref(), Some("100.64.0.1"));
        assert_eq!(back.port, 11434);
        assert_eq!(back.bearer.as_deref(), Some("tok_abc"));
    }

    #[tokio::test]
    async fn resolve_returns_none_when_nothing_reachable() {
        // Use TEST-NET addresses that are guaranteed not to be reachable.
        let ep = RemoteEndpoint {
            lan: Some("192.0.2.1".into()),
            tailscale: Some("192.0.2.2".into()),
            port: 19999,
            bearer: None,
        };
        let result = ep.resolve_base_url().await;
        assert!(result.is_none(), "expected None for unreachable addresses");
    }

    #[tokio::test]
    async fn resolve_returns_none_when_no_addresses_configured() {
        let ep = RemoteEndpoint::default();
        let result = ep.resolve_base_url().await;
        assert!(result.is_none());
    }
}
