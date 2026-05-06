//! mDNS advertiser + browser for `_ferriscribe._tcp.local.`.

use std::collections::HashMap;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

pub const SERVICE_TYPE: &str = "_ferriscribe._tcp.local.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredServer {
    pub instance_name: String,
    pub host: String,
    pub addresses: Vec<String>,
    pub ports: ServerPorts,
    pub version: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerPorts {
    pub ollama: Option<u16>,
    pub whisper: Option<u16>,
    pub lmstudio: Option<u16>,
    pub pairing: Option<u16>,
    pub vocab: Option<u16>,
}

pub struct MdnsAdvertiser {
    daemon: ServiceDaemon,
    fullname: String,
}

impl MdnsAdvertiser {
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

    pub fn stop(self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
    }
}

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
