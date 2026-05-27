//! mDNS advertiser and browser for `_ferriscribe._tcp.local.`.
//!
//! ## Server side
//!
//! [`MdnsAdvertiser::start`] registers a service record with TXT properties
//! for each proxy port (ollama, whisper, lmstudio, pairing, vocab) and the
//! crate version. The advertiser uses `enable_addr_auto()` so the daemon
//! picks the best interface address automatically.
//!
//! ## Client side
//!
//! [`browse`] spawns a background task that listens for `ServiceResolved`
//! events and sends [`DiscoveredServer`] values through a channel until the
//! timeout elapses.

use std::collections::HashMap;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// mDNS service type used for FerriScribe server discovery.
pub const SERVICE_TYPE: &str = "_ferriscribe._tcp.local.";

/// A FerriScribe server discovered via mDNS or Tailscale probing.
///
/// Sent through the channel returned by [`browse`]. The frontend uses this
/// to populate the "available servers" list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredServer {
    /// Full mDNS instance name (e.g. `"Clinic Server._ferriscribe._tcp.local."`).
    pub instance_name: String,
    /// Hostname (trailing dot stripped).
    pub host: String,
    /// Addresses learned via mDNS broadcast (LAN multicast).
    pub addresses: Vec<String>,
    /// Addresses learned via Tailscale peer enumeration. Kept separate from
    /// `addresses` so the frontend can route them into the `tailscale` slot
    /// of `RemoteEndpoint` instead of misclassifying them as LAN hosts.
    #[serde(default)]
    pub tailscale_addresses: Vec<String>,
    /// Service ports advertised in TXT records.
    pub ports: ServerPorts,
    /// Crate version string from the mDNS TXT record.
    pub version: String,
}

/// Service ports advertised by a FerriScribe server.
///
/// All fields are `Option` because a server may not have every subsystem
/// enabled (e.g. no LM Studio detected). Used both in mDNS TXT records and
/// in the `/info` HTTP endpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerPorts {
    /// Ollama auth proxy port.
    pub ollama: Option<u16>,
    /// Whisper auth proxy port.
    pub whisper: Option<u16>,
    /// LM Studio auth proxy port (absent when LM Studio wasn't detected).
    pub lmstudio: Option<u16>,
    /// Pairing HTTP service port.
    pub pairing: Option<u16>,
    /// Vocabulary CRUD HTTP API port.
    pub vocab: Option<u16>,
}

/// mDNS service advertiser.
///
/// Registers a `_ferriscribe._tcp.local.` service record with TXT properties
/// encoding the server's proxy ports and version. Drop or call [`stop`] to
/// unregister.
pub struct MdnsAdvertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl MdnsAdvertiser {
    /// Start advertising a FerriScribe server on the local network.
    ///
    /// The `instance_name` appears in clients' discovery lists. Service ports
    /// and version are published as TXT records. The advertised listener
    /// port is `ports.pairing` (falling back to 11436) since that's the
    /// endpoint clients need to contact first.
    pub fn start(
        instance_name: &str,
        ports: &ServerPorts,
        version: &str,
    ) -> crate::Result<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| crate::SharingError::Mdns(e.to_string()))?;
        let host = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "localhost".to_string());
        let host_with_dot = if host.ends_with(".local.") {
            host.clone()
        } else {
            format!("{host}.local.")
        };
        let mut props: HashMap<String, String> = HashMap::new();
        if let Some(p) = ports.ollama {
            props.insert("ollama".into(), p.to_string());
        }
        if let Some(p) = ports.whisper {
            props.insert("whisper".into(), p.to_string());
        }
        if let Some(p) = ports.lmstudio {
            props.insert("lmstudio".into(), p.to_string());
        }
        if let Some(p) = ports.pairing {
            props.insert("pairing".into(), p.to_string());
        }
        if let Some(p) = ports.vocab {
            props.insert("vocab".into(), p.to_string());
        }
        props.insert("version".into(), version.to_string());
        let advertise_port = ports.pairing.unwrap_or(11436);
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            instance_name,
            &host_with_dot,
            "",
            advertise_port,
            Some(props),
        )
        .map_err(|e| crate::SharingError::Mdns(e.to_string()))?
        .enable_addr_auto();
        daemon.register(info.clone())
            .map_err(|e| crate::SharingError::Mdns(e.to_string()))?;
        Ok(Self {
            daemon,
            fullname: format!("{instance_name}.{SERVICE_TYPE}"),
        })
    }

    /// Unregister the service and shut down the mDNS daemon.
    pub fn stop(self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

/// Browse for FerriScribe servers on the local network.
///
/// Spawns a background task that listens for mDNS `ServiceResolved` events
/// and sends [`DiscoveredServer`] values through the returned channel. The
/// background task exits after `timeout` elapses, at which point the
/// channel closes and `recv()` returns `None`.
///
/// # Example
///
/// ```rust,no_run
/// use std::time::Duration;
/// use medical_sharing::mdns;
///
/// let mut rx = mdns::browse(Duration::from_secs(5))?;
/// while let Some(server) = rx.recv().await {
///     println!("found: {}", server.instance_name);
/// }
/// # Ok::<(), medical_sharing::SharingError>(())
/// ```
pub fn browse(timeout: Duration) -> crate::Result<mpsc::Receiver<DiscoveredServer>> {
    let daemon = ServiceDaemon::new()
        .map_err(|e| crate::SharingError::Mdns(e.to_string()))?;
    let receiver = daemon
        .browse(SERVICE_TYPE)
        .map_err(|e| crate::SharingError::Mdns(e.to_string()))?;
    let (tx, rx) = mpsc::channel::<DiscoveredServer>(32);
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + timeout;
        while tokio::time::Instant::now() < deadline {
            match receiver.recv_async().await {
                Ok(ServiceEvent::ServiceResolved(info)) => {
                    let props = info.get_properties();
                    let prop = |k: &str| props.get_property_val_str(k).map(|s| s.to_string());
                    let parse_port = |k: &str| prop(k).and_then(|s| s.parse::<u16>().ok());
                    let server = DiscoveredServer {
                        instance_name: info.get_fullname().to_string(),
                        host: info.get_hostname().trim_end_matches('.').to_string(),
                        addresses: info
                            .get_addresses()
                            .iter()
                            .map(|a| a.to_string())
                            .collect(),
                        tailscale_addresses: Vec::new(),
                        ports: ServerPorts {
                            ollama: parse_port("ollama"),
                            whisper: parse_port("whisper"),
                            lmstudio: parse_port("lmstudio"),
                            pairing: parse_port("pairing"),
                            vocab: parse_port("vocab"),
                        },
                        version: prop("version").unwrap_or_default(),
                    };
                    if tx.send(server).await.is_err() {
                        break;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let _ = daemon.shutdown();
    });
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn advertise_then_browse_finds_self() {
        if std::env::var("FERRISCRIBE_MDNS_TEST").ok().as_deref() != Some("1") {
            eprintln!("skipping: set FERRISCRIBE_MDNS_TEST=1 to run mDNS smoke test");
            return;
        }
        let ports = ServerPorts {
            ollama: Some(11435),
            whisper: Some(8081),
            lmstudio: None,
            pairing: Some(11436),
            vocab: Some(11437),
        };
        let adv = MdnsAdvertiser::start("test-instance", &ports, "0.0.0.0").unwrap();
        // Give the daemon a moment to publish.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut rx = browse(Duration::from_secs(3)).unwrap();
        let mut found = None;
        while let Some(d) = rx.recv().await {
            if d.instance_name.contains("test-instance") {
                found = Some(d);
                break;
            }
        }
        adv.stop();
        let d = found.expect("did not discover own advertisement");
        assert_eq!(d.ports.ollama, Some(11435));
        assert_eq!(d.ports.pairing, Some(11436));
    }
}
