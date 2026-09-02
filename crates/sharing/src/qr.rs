//! Encode the `ferriscribe://pair?...` URL the QR carries.
//!
//! The URL is a custom-scheme deep link that the FerriScribe client app
//! recognises. It encodes the server's hostname, LAN/Tailscale addresses,
//! all proxy ports, and the 6-digit pairing code. Decoding happens in the
//! client's TypeScript (`usePairing.svelte.ts`), so only `encode` lives
//! here.
//!
//! ## URL format
//!
//! ```text
//! ferriscribe://pair?code=042917&host=Clinic+Server&lan=192.168.1.42
//!     &op=11435&wp=8081&pp=11436&ts=clinic.tail-abc.ts.net&vp=11437
//! ```
//!
//! Query params use a `BTreeMap` for deterministic key ordering (easier
//! visual diffing of QR payloads).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Payload encoded in a `ferriscribe://pair?...` QR URL.
///
/// Contains everything a client needs to connect: server identity, network
/// addresses (LAN and/or Tailscale), all service ports, and the pairing code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairPayload {
    /// Human-readable server name (shown in the client's server picker).
    pub host: String,
    /// LAN IPv4 address (absent when the server is Tailscale-only).
    pub lan: Option<String>,
    /// Tailscale DNS name (absent when the server is LAN-only).
    pub tailscale: Option<String>,
    /// Service ports for all proxy endpoints.
    pub ports: PairPorts,
    /// 6-digit pairing code.
    pub code: String,
}

/// Service ports carried in the QR payload.
///
/// `lmstudio`, `omlx`, and `vocab` are optional because not every server runs
/// those subsystems. A missing `vocab` port means "vocab sync unavailable" and
/// clients fall back to local vocabulary.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairPorts {
    /// Ollama auth proxy port (query param `op`).
    pub ollama: u16,
    /// Whisper auth proxy port (query param `wp`).
    pub whisper: u16,
    /// Pairing HTTP service port (query param `pp`).
    pub pairing: u16,
    /// LM Studio auth proxy port (query param `lp`). Absent when LM Studio isn't running.
    pub lmstudio: Option<u16>,
    /// oMLX auth proxy port (query param `mp`). Absent when oMLX isn't running.
    #[serde(default)]
    pub omlx: Option<u16>,
    /// Vocabulary CRUD HTTP API port (query param `vp`). `None` when the
    /// office server predates the vocab-sync feature; clients should treat
    /// absence as "vocab sync unavailable" and fall back to local vocab.
    #[serde(default)]
    pub vocab: Option<u16>,
}

/// Encode a [`PairPayload`] into a `ferriscribe://pair?...` URL string.
///
/// Keys are emitted in sorted order (via `BTreeMap`) for deterministic
/// output. Values are percent-encoded.
pub fn encode(p: &PairPayload) -> String {
    let mut q: BTreeMap<&'static str, String> = BTreeMap::new();
    q.insert("host", p.host.clone());
    if let Some(l) = &p.lan {
        q.insert("lan", l.clone());
    }
    if let Some(t) = &p.tailscale {
        q.insert("ts", t.clone());
    }
    q.insert("op", p.ports.ollama.to_string());
    q.insert("wp", p.ports.whisper.to_string());
    q.insert("pp", p.ports.pairing.to_string());
    if let Some(l) = p.ports.lmstudio {
        q.insert("lp", l.to_string());
    }
    if let Some(m) = p.ports.omlx {
        q.insert("mp", m.to_string());
    }
    if let Some(v) = p.ports.vocab {
        q.insert("vp", v.to_string());
    }
    q.insert("code", p.code.clone());
    let qs: Vec<String> = q
        .into_iter()
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(&v)))
        .collect();
    format!("ferriscribe://pair?{}", qs.join("&"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PairPayload {
        PairPayload {
            host: "Clinic Server".to_string(),
            lan: Some("192.168.1.42".to_string()),
            tailscale: Some("clinic.tail-abc.ts.net".to_string()),
            ports: PairPorts {
                ollama: 11435,
                whisper: 8081,
                pairing: 11436,
                lmstudio: Some(1235),
                omlx: Some(8001),
                vocab: Some(11437),
            },
            code: "123456".to_string(),
        }
    }

    #[test]
    fn encodes_all_params_in_sorted_order() {
        let url = encode(&sample());
        // Sorted keys: code, host, lan, lp, mp, op, pp, ts, vp, wp. Spaces
        // percent-encode as %20 (urlencoding crate).
        assert!(url.starts_with("ferriscribe://pair?code=123456&host=Clinic%20Server"));
        assert!(url.contains("lan=192.168.1.42"));
        assert!(url.contains("op=11435"));
        assert!(url.contains("wp=8081"));
        assert!(url.contains("pp=11436"));
        assert!(url.contains("lp=1235"));
        assert!(url.contains("mp=8001"));
        assert!(url.contains("vp=11437"));
        assert!(url.contains("ts=clinic.tail-abc.ts.net"));
    }

    #[test]
    fn omits_absent_optional_ports() {
        // Pre-oMLX shape: no lp/mp/vp params at all.
        let mut p = sample();
        p.ports.lmstudio = None;
        p.ports.omlx = None;
        p.ports.vocab = None;
        let url = encode(&p);
        assert!(!url.contains("lp="));
        assert!(!url.contains("mp="));
        assert!(!url.contains("vp="));
        assert!(url.contains("op=11435"));
    }
}
