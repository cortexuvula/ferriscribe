//! Office-server discovery: mDNS browse plus a Tailscale-aware peer probe.
//!
//! mDNS doesn't traverse Tailscale (link-layer multicast vs. overlay routing),
//! so cross-network paired clients can't see the office server's broadcasts.
//! The Tailscale path enumerates tailnet peers via `tailscale status --json`
//! and probes each at `:11436/info`.

use medical_core::error::{AppError, AppResult};
use medical_sharing::mdns::DiscoveredServer;
use serde::Deserialize;

#[tauri::command]
pub async fn discover_servers(timeout_ms: u64) -> AppResult<Vec<DiscoveredServer>> {
    let mut rx =
        medical_sharing::mdns::browse(std::time::Duration::from_millis(timeout_ms))
            .map_err(|e| AppError::Other(e.to_string()))?;
    let mut out = Vec::new();
    while let Some(d) = rx.recv().await {
        out.push(d);
    }
    Ok(out)
}

/// Discover FerriScribe office servers among the local Tailscale tailnet's
/// peers. Peers that respond with a parseable InfoSnapshot are returned shaped
/// like an mDNS DiscoveredServer so the frontend can merge both lists into
/// the same UI.
#[tauri::command]
pub async fn discover_via_tailscale(
    timeout_ms: u64,
) -> AppResult<Vec<DiscoveredServer>> {
    let peers = tailscale_peers().await.unwrap_or_default();
    if peers.is_empty() {
        return Ok(Vec::new());
    }
    let probe_timeout = std::time::Duration::from_millis(timeout_ms.max(1000));
    let client = match reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_millis(800))
        .timeout(probe_timeout)
        .build()
    {
        Ok(c) => c,
        Err(_) => return Ok(Vec::new()),
    };

    let probes = peers.into_iter().map(|peer| {
        let client = client.clone();
        async move {
            let url = format!("http://{}:11436/info", peer.dial);
            let resp = client.get(&url).send().await.ok()?;
            if !resp.status().is_success() {
                return None;
            }
            let info: InfoSnapshotWire = resp.json().await.ok()?;
            Some(DiscoveredServer {
                instance_name: format!("{}._ferriscribe._tcp.local.", info.host),
                host: peer.host,
                addresses: vec![peer.dial],
                ports: medical_sharing::mdns::ServerPorts {
                    ollama: info.ports.ollama,
                    whisper: info.ports.whisper,
                    lmstudio: info.ports.lmstudio,
                    pairing: info.ports.pairing,
                    vocab: info.ports.vocab,
                },
                version: info.version,
            })
        }
    });
    let results: Vec<Option<DiscoveredServer>> = futures_util::future::join_all(probes).await;
    Ok(results.into_iter().flatten().collect())
}

#[derive(Debug, Clone)]
struct TailscalePeer {
    /// MagicDNS hostname or first Tailscale IP — whichever is more useful for
    /// dialing. Stored as `lan-style` host without scheme/port.
    dial: String,
    /// Best-effort display name (the peer's hostname).
    host: String,
}

async fn tailscale_peers() -> Option<Vec<TailscalePeer>> {
    let out = tokio::process::Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let peer_obj = v.get("Peer")?.as_object()?;
    let mut peers = Vec::new();
    for (_, p) in peer_obj {
        // Skip peers we can't reach (offline, awaiting auth, etc.).
        if p.get("Online").and_then(|x| x.as_bool()) != Some(true) {
            continue;
        }
        let host = p
            .get("HostName")
            .and_then(|x| x.as_str())
            .unwrap_or("(unknown)")
            .to_string();
        let dns = p.get("DNSName").and_then(|x| x.as_str()).map(|s| {
            // tailscale's DNSName is like "host.tailnet.ts.net." — strip trailing dot.
            s.trim_end_matches('.').to_string()
        });
        let first_ip = p
            .get("TailscaleIPs")
            .and_then(|x| x.as_array())
            .and_then(|a| a.first())
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        // Prefer DNS name (works through Tailscale's name resolver and survives
        // IP changes), fall back to first IP.
        let dial = dns.clone().or(first_ip)?;
        peers.push(TailscalePeer { dial, host });
    }
    Some(peers)
}

#[derive(Debug, Deserialize)]
struct InfoSnapshotWire {
    host: String,
    version: String,
    ports: WirePorts,
}

#[derive(Debug, Deserialize)]
struct WirePorts {
    #[serde(default)]
    ollama: Option<u16>,
    #[serde(default)]
    whisper: Option<u16>,
    #[serde(default)]
    lmstudio: Option<u16>,
    #[serde(default)]
    pairing: Option<u16>,
    #[serde(default)]
    vocab: Option<u16>,
}
