//! Encode and decode the `ferriscribe://pair?...` URL the QR carries.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairPayload {
    pub host: String,
    pub lan: Option<String>,
    pub tailscale: Option<String>,
    pub ports: PairPorts,
    pub code: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairPorts {
    pub ollama: u16,
    pub whisper: u16,
    pub pairing: u16,
    pub lmstudio: Option<u16>,
    /// Vocabulary CRUD HTTP API. None when the office server predates
    /// the vocab-sync feature; clients should treat absence as "vocab
    /// sync unavailable" and fall back to local vocab.
    #[serde(default)]
    pub vocab: Option<u16>,
}

pub fn encode(p: &PairPayload) -> String {
    let mut q: BTreeMap<&'static str, String> = BTreeMap::new();
    q.insert("host", p.host.clone());
    if let Some(l) = &p.lan { q.insert("lan", l.clone()); }
    if let Some(t) = &p.tailscale { q.insert("ts", t.clone()); }
    q.insert("op", p.ports.ollama.to_string());
    q.insert("wp", p.ports.whisper.to_string());
    q.insert("pp", p.ports.pairing.to_string());
    if let Some(l) = p.ports.lmstudio { q.insert("lp", l.to_string()); }
    if let Some(v) = p.ports.vocab { q.insert("vp", v.to_string()); }
    q.insert("code", p.code.clone());
    let qs: Vec<String> = q
        .into_iter()
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(&v)))
        .collect();
    format!("ferriscribe://pair?{}", qs.join("&"))
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("not a ferriscribe pairing URL")]
    NotPairUrl,
    #[error("missing field: {0}")]
    Missing(&'static str),
    #[error("bad number: {0}")]
    BadNumber(String),
}

pub fn decode(url: &str) -> Result<PairPayload, DecodeError> {
    let rest = url.strip_prefix("ferriscribe://pair?").ok_or(DecodeError::NotPairUrl)?;
    let mut map = std::collections::HashMap::<String, String>::new();
    for kv in rest.split('&') {
        if let Some((k, v)) = kv.split_once('=') {
            map.insert(k.to_string(), urlencoding::decode(v).unwrap_or_default().into_owned());
        }
    }
    let parse_port = |s: &str| -> Result<u16, DecodeError> {
        s.parse().map_err(|e: std::num::ParseIntError| DecodeError::BadNumber(e.to_string()))
    };
    let host    = map.remove("host").ok_or(DecodeError::Missing("host"))?;
    let lan     = map.remove("lan");
    let tailscale = map.remove("ts");
    let op      = map.remove("op").ok_or(DecodeError::Missing("op"))?;
    let wp      = map.remove("wp").ok_or(DecodeError::Missing("wp"))?;
    let pp      = map.remove("pp").ok_or(DecodeError::Missing("pp"))?;
    let lp      = map.remove("lp").and_then(|s| s.parse().ok());
    let vp      = map.remove("vp").and_then(|s| s.parse().ok());
    let code    = map.remove("code").ok_or(DecodeError::Missing("code"))?;
    Ok(PairPayload {
        host,
        lan,
        tailscale,
        ports: PairPorts {
            ollama:   parse_port(&op)?,
            whisper:  parse_port(&wp)?,
            pairing:  parse_port(&pp)?,
            lmstudio: lp,
            vocab:    vp,
        },
        code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let p = PairPayload {
            host: "Clinic Server".to_string(),
            lan: Some("192.168.1.42".to_string()),
            tailscale: Some("clinic.tail-abc.ts.net".to_string()),
            ports: PairPorts { ollama: 11435, whisper: 8081, pairing: 11436, lmstudio: Some(1234), vocab: Some(11437) },
            code: "123456".to_string(),
        };
        let url = encode(&p);
        let back = decode(&url).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode("https://example.com").is_err());
    }
}
