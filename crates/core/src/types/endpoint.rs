//! `RemoteEndpoint` — LAN/Tailscale connection resolver with optional bearer auth.

use crate::error::OfflineReason;

/// Build an HTTP URL from a host and port, bracketing IPv6 literals as
/// required by RFC 3986.
///
/// Without this, `http://fe80::1:8080` parses as host `fe80` with three
/// nested ports — reqwest then emits a "Builder error" rather than a
/// useful message. Hostnames and IPv4 literals pass through unchanged.
/// Already-bracketed hosts are not double-bracketed.
pub fn http_url(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("http://[{host}]:{port}")
    } else {
        format!("http://{host}:{port}")
    }
}

/// A remote endpoint that may be reachable on either a LAN address or a
/// Tailscale address.
///
/// The resolver ([`resolve_base_url`](RemoteEndpoint::resolve_base_url))
/// probes the LAN address first with a 500 ms connect timeout, then
/// falls back to the Tailscale address with a 2 s timeout. Used by the
/// `sharing` crate for connecting to remote FerriScribe instances.
///
/// The custom `Debug` impl redacts the bearer token so it can never
/// appear in `tracing::debug!(?endpoint, …)` output.
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
/// This is a PHI/security requirement — bearer tokens must not leak to logs.
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
    /// Probe the LAN address with a 500 ms connect timeout, then fall
    /// back to the Tailscale address (2 s timeout).
    ///
    /// Returns `Ok` with the URL prefix (e.g. `"http://192.168.1.42:11435"`)
    /// for the first reachable address, or `Err` with the probe outcome
    /// classified into the user-facing [`OfflineReason`].
    ///
    /// Fallback semantics (regression fix, review cc9b532): a LAN failure
    /// STILL probes Tailscale — a roaming user off the clinic network must
    /// connect over Tailscale, not see a spurious offline dialog. The LAN
    /// reason is reported only when Tailscale also fails (LAN is the
    /// primary address; its failure mode is what the user should act on).
    ///
    /// The classification mirrors [`classify_probe_failure`], which is the
    /// TCP-probe counterpart of the HTTP-probe classifier
    /// `medical_core::preflight::classify_reqwest_error` — a refused
    /// connection, a failed name resolution and a genuine timeout produce
    /// distinct `OfflineReason`s that the offline dialog renders as
    /// distinct messages.
    pub async fn resolve_base_url(&self) -> Result<String, ProbeFailure> {
        let mut lan_failure: Option<ProbeFailure> = None;
        if let Some(lan) = self.lan.as_deref() {
            match Self::probe_host(lan, self.port, std::time::Duration::from_millis(500)).await {
                Ok(()) => return Ok(http_url(lan, self.port)),
                Err(reason) => {
                    lan_failure = Some(ProbeFailure {
                        host: Some(lan.to_string()),
                        reason,
                    });
                    // Fall through: Tailscale is still probed.
                }
            }
        }
        if let Some(ts) = self.tailscale.as_deref() {
            match Self::probe_host(ts, self.port, std::time::Duration::from_secs(2)).await {
                Ok(()) => return Ok(http_url(ts, self.port)),
                Err(_ts_reason) => {
                    // Both failed: report the LAN outcome when LAN was
                    // configured (primary address); else the Tailscale one.
                    return Err(lan_failure.unwrap_or(ProbeFailure {
                        host: Some(ts.to_string()),
                        reason: _ts_reason,
                    }));
                }
            }
        }
        // No Tailscale configured (or none probed): the LAN failure stands.
        Err(lan_failure.unwrap_or(ProbeFailure {
            host: None,
            reason: OfflineReason::ConnectionRefused,
        }))
    }

    /// Probe one host/port, distinguishing the failure classes that a bare
    /// `bool` collapsed: a connect error (refused / unreachable / DNS) is
    /// inspected via [`classify_probe_failure`], while only a genuinely
    /// elapsed deadline yields [`OfflineReason::Timeout`].
    async fn probe_host(
        host: &str,
        port: u16,
        timeout: std::time::Duration,
    ) -> Result<(), OfflineReason> {
        match tokio::time::timeout(timeout, tokio::net::TcpStream::connect((host, port))).await {
            // Outer Err: the deadline elapsed before the connect resolved.
            Ok(connect) => match connect {
                Ok(_stream) => Ok(()),
                Err(e) => Err(classify_probe_failure(&e)),
            },
            Err(_elapsed) => Err(OfflineReason::Timeout),
        }
    }
}

/// What a TCP probe failure means for the user, in the vocabulary the
/// offline dialog already renders ([`OfflineReason`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeFailure {
    /// Host that the failed probe targeted (LAN first, else Tailscale;
    /// `None` when no address was configured at all).
    pub host: Option<String>,
    /// Classified reason the probe failed.
    pub reason: OfflineReason,
}

/// Classify a failed [`tokio::net::TcpStream::connect`] into an
/// [`OfflineReason`].
///
/// This is the TCP-probe counterpart of
/// `medical_core::preflight::classify_reqwest_error` (the HTTP classifier
/// referenced by `stt-providers/src/client.rs`): DNS resolution failures
/// are pulled out of the io error text so they stop masquerading as
/// timeouts, and everything else — most importantly an actively refused
/// connection — is a connection error, not a timeout. `is_timeout()` is
/// only true for an actual elapsed deadline, which `probe_host` already
/// reports directly, so a `Timeout` here means the io layer itself timed
/// out internally.
pub fn classify_probe_failure(err: &std::io::Error) -> OfflineReason {
    if matches!(
        err.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        return OfflineReason::Timeout;
    }
    let msg = err.to_string().to_lowercase();
    if err.kind() == std::io::ErrorKind::NotConnected
        || msg.contains("dns")
        || msg.contains("failed to lookup")
        || msg.contains("nodename nor servname")
        || msg.contains("name or service not known")
        || msg.contains("temporary failure in name resolution")
    {
        return OfflineReason::DnsFailure;
    }
    OfflineReason::ConnectionRefused
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

    /// Regression (review cc9b532): LAN configured but dead MUST still
    /// fall back to a listening Tailscale address — the broken version
    /// returned the LAN error immediately and a roaming user saw a
    /// spurious offline dialog.
    ///
    /// Setup uses the two loopback stacks to fake distinct hosts on one
    /// port number: LAN probes IPv4 127.0.0.1 (nothing listening → the
    /// port is closed), Tailscale probes IPv6 ::1 where a listener IS
    /// bound. If the fallback is broken, the LAN refusal wins and this
    /// test fails with ConnectionRefused.
    #[tokio::test]
    async fn lan_dead_tailscale_listening_resolves_to_tailscale() {
        use std::net::TcpListener;
        // Pick a port free on BOTH loopback stacks.
        let probe = TcpListener::bind(("::1", 0)).unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let v6_listener = TcpListener::bind(("::1", port)).unwrap();

        let ep = RemoteEndpoint {
            lan: Some("127.0.0.1".into()), // IPv4 loopback: dead
            tailscale: Some("::1".into()), // IPv6 loopback: listening
            port,
            bearer: None,
        };
        let result = ep.resolve_base_url().await.unwrap();
        assert_eq!(
            result,
            format!("http://[::1]:{port}"),
            "dead LAN must fall back to listening Tailscale"
        );
        drop(v6_listener);
    }

    /// Regression (review cc9b532): when BOTH LAN and Tailscale are refused,
    /// the representative failure is the LAN address (primary), not Tailscale.
    #[tokio::test]
    async fn lan_and_tailscale_both_refused_reports_lan_reason() {
        use std::net::TcpListener;
        // Nothing is bound on either loopback address -> immediate refusal.
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        let ep = RemoteEndpoint {
            lan: Some("127.0.0.1".into()),       // refused
            tailscale: Some("127.0.0.2".into()), // refused (loopback, nothing bound)
            port,
            bearer: None,
        };
        let err = ep.resolve_base_url().await.unwrap_err();
        assert_eq!(
            err.host.as_deref(),
            Some("127.0.0.1"),
            "LAN is the representative failure when both are refused"
        );
        assert_eq!(err.reason, OfflineReason::ConnectionRefused);
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
        // Probes never complete against TEST-NET, so both deadlines elapse.
        // The LAN probe is representative.
        assert_eq!(
            result,
            Err(ProbeFailure {
                host: Some("192.0.2.1".into()),
                reason: OfflineReason::Timeout,
            }),
            "expected Timeout for addresses that never answer"
        );
    }

    #[tokio::test]
    async fn resolve_returns_none_when_no_addresses_configured() {
        let ep = RemoteEndpoint::default();
        let result = ep.resolve_base_url().await;
        assert_eq!(
            result,
            Err(ProbeFailure {
                host: None,
                reason: OfflineReason::ConnectionRefused,
            })
        );
    }

    /// The B6 regression trio: a refused connection, a failed name
    /// resolution and a genuinely timed-out probe must surface as three
    /// DISTINCT `OfflineReason`s (the offline dialog renders a different
    /// sentence for each) instead of all collapsing into `Timeout`.
    mod classification {
        use super::*;

        fn dead_port() -> u16 {
            // Bind then immediately drop to get a free port that is
            // guaranteed closed — connects are actively refused.
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            port
        }

        #[tokio::test]
        async fn refused_connection_is_connection_refused_not_timeout() {
            let port = dead_port();
            let ep = RemoteEndpoint {
                lan: Some("127.0.0.1".into()),
                tailscale: None,
                port,
                bearer: None,
            };
            let result = ep.resolve_base_url().await;
            assert_eq!(
                result,
                Err(ProbeFailure {
                    host: Some("127.0.0.1".into()),
                    reason: OfflineReason::ConnectionRefused,
                }),
                "an actively refused connect must not be reported as a timeout"
            );
        }

        #[tokio::test]
        async fn unresolvable_host_is_dns_failure_not_timeout() {
            // `.invalid` is reserved by RFC 2606 to always fail DNS.
            let ep = RemoteEndpoint {
                lan: Some("nonexistent-host.invalid".into()),
                tailscale: None,
                port: 8080,
                bearer: None,
            };
            let result = ep.resolve_base_url().await;
            assert_eq!(
                result,
                Err(ProbeFailure {
                    host: Some("nonexistent-host.invalid".into()),
                    reason: OfflineReason::DnsFailure,
                }),
                "a failed name resolution must not be reported as a timeout"
            );
        }

        #[tokio::test]
        async fn silent_host_is_timeout() {
            // TEST-NET-1 never answers; the probe deadline must elapse and
            // surface as a genuine Timeout.
            let ep = RemoteEndpoint {
                lan: Some("192.0.2.1".into()),
                tailscale: None,
                port: 19999,
                bearer: None,
            };
            let result = ep.resolve_base_url().await;
            assert_eq!(
                result,
                Err(ProbeFailure {
                    host: Some("192.0.2.1".into()),
                    reason: OfflineReason::Timeout,
                })
            );
        }
    }

    /// The classifier must agree with the vocabulary that the HTTP-side
    /// classifier (`preflight::classify_reqwest_error`) established:
    /// refused / DNS / timeout are distinct user-facing outcomes.
    #[test]
    fn classify_probe_failure_distinguishes_kinds() {
        use std::io::{Error, ErrorKind};

        let refused = Error::from_raw_os_error(61); // ECONNREFUSED (macOS)
        assert_eq!(
            classify_probe_failure(&refused),
            OfflineReason::ConnectionRefused
        );

        let dns = Error::new(
            ErrorKind::NotConnected,
            "failed to lookup address information: nodename nor servname provided",
        );
        assert_eq!(classify_probe_failure(&dns), OfflineReason::DnsFailure);

        let dns_windows = Error::new(
            ErrorKind::NotConnected,
            "failed to lookup address information: Name or service not known",
        );
        assert_eq!(
            classify_probe_failure(&dns_windows),
            OfflineReason::DnsFailure
        );

        let io_timeout = Error::new(ErrorKind::TimedOut, "connection timed out");
        assert_eq!(classify_probe_failure(&io_timeout), OfflineReason::Timeout);

        let unreachable = Error::from_raw_os_error(65); // EHOSTUNREACH
        assert_eq!(
            classify_probe_failure(&unreachable),
            OfflineReason::ConnectionRefused
        );
    }
}
