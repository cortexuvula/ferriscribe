//! Pure-function helpers that mutate AppConfig when pairing / unpairing.
//! Extracted so pair/unpair logic is unit-testable without DB or keychain.

use medical_core::types::settings::{AppConfig, SttMode};
use medical_sharing::qr::PairPorts;

/// Apply the office server's resolved address + ports to AppConfig.
/// Preserves `cfg.ai_provider` — pair does NOT change which provider is active
/// (the separate availability check in `pair_with_server` handles the case
/// where the server doesn't serve the current provider). LM Studio / oMLX
/// fields are only touched when their ports are `Some`.
pub fn apply_paired_settings(cfg: &mut AppConfig, host: &str, ports: &PairPorts) {
    cfg.stt_mode = SttMode::Remote;
    cfg.stt_remote_host = host.to_string();
    cfg.stt_remote_port = ports.whisper;
    cfg.ollama_host = host.to_string();
    cfg.ollama_port = ports.ollama;
    if let Some(lp) = ports.lmstudio {
        cfg.lmstudio_host = host.to_string();
        cfg.lmstudio_port = lp;
    }
    if let Some(mp) = ports.omlx {
        cfg.omlx_host = host.to_string();
        cfg.omlx_port = mp;
    }
}

/// Reset the AppConfig fields the pair flow populated, back to local defaults.
/// Preserves `cfg.ai_provider`.
pub fn reset_paired_settings(cfg: &mut AppConfig) {
    cfg.stt_mode = SttMode::Local;
    cfg.stt_remote_host = String::new();
    cfg.stt_remote_port = 8080;
    cfg.ollama_host = "localhost".into();
    cfg.ollama_port = 11434;
    cfg.lmstudio_host = "localhost".into();
    cfg.lmstudio_port = 1234;
    cfg.omlx_host = "localhost".into();
    cfg.omlx_port = 8000;
}

#[cfg(test)]
mod tests {
    use super::*;
    use medical_core::types::settings::AppConfig;

    fn ports(lmstudio: Option<u16>) -> PairPorts {
        ports_with(lmstudio, None)
    }

    fn ports_with(lmstudio: Option<u16>, omlx: Option<u16>) -> PairPorts {
        PairPorts {
            ollama: 11435,
            whisper: 8081,
            pairing: 11436,
            lmstudio,
            omlx,
            vocab: Some(11437),
        }
    }

    #[test]
    fn apply_paired_settings_populates_all_three_services_when_lmstudio_present() {
        let mut cfg = AppConfig::default();
        cfg.ai_provider = "lmstudio".into();
        apply_paired_settings(&mut cfg, "192.168.4.37", &ports(Some(1235)));

        assert_eq!(cfg.stt_mode, SttMode::Remote);
        assert_eq!(cfg.stt_remote_host, "192.168.4.37");
        assert_eq!(cfg.stt_remote_port, 8081);
        assert_eq!(cfg.ollama_host, "192.168.4.37");
        assert_eq!(cfg.ollama_port, 11435);
        assert_eq!(cfg.lmstudio_host, "192.168.4.37");
        assert_eq!(cfg.lmstudio_port, 1235);
        assert_eq!(cfg.ai_provider, "lmstudio", "ai_provider must be preserved");
    }

    #[test]
    fn apply_paired_settings_leaves_lmstudio_fields_alone_when_port_is_none() {
        let mut cfg = AppConfig::default();
        let original_lmstudio_host = cfg.lmstudio_host.clone();
        let original_lmstudio_port = cfg.lmstudio_port;

        apply_paired_settings(&mut cfg, "192.168.4.37", &ports(None));

        assert_eq!(cfg.lmstudio_host, original_lmstudio_host);
        assert_eq!(cfg.lmstudio_port, original_lmstudio_port);
        assert_eq!(cfg.stt_remote_host, "192.168.4.37");
        assert_eq!(cfg.ollama_host, "192.168.4.37");
    }

    #[test]
    fn apply_paired_settings_populates_omlx_when_port_present() {
        let mut cfg = AppConfig::default();
        apply_paired_settings(&mut cfg, "192.168.4.37", &ports_with(None, Some(8001)));

        assert_eq!(cfg.omlx_host, "192.168.4.37");
        assert_eq!(cfg.omlx_port, 8001);
        assert_eq!(
            cfg.lmstudio_host, "localhost",
            "lmstudio untouched when its port is absent"
        );
    }

    #[test]
    fn apply_paired_settings_leaves_omlx_fields_alone_when_port_is_none() {
        let mut cfg = AppConfig::default();
        cfg.omlx_host = "10.9.9.9".into();
        cfg.omlx_port = 9000;
        apply_paired_settings(&mut cfg, "192.168.4.37", &ports(None));

        assert_eq!(cfg.omlx_host, "10.9.9.9");
        assert_eq!(cfg.omlx_port, 9000);
    }

    #[test]
    fn apply_paired_settings_preserves_ai_provider() {
        let mut cfg = AppConfig::default();
        cfg.ai_provider = "ollama".into();
        apply_paired_settings(&mut cfg, "10.0.0.5", &ports(Some(1235)));
        assert_eq!(cfg.ai_provider, "ollama");

        let mut cfg2 = AppConfig::default();
        cfg2.ai_provider = "lmstudio".into();
        apply_paired_settings(&mut cfg2, "10.0.0.5", &ports(Some(1235)));
        assert_eq!(cfg2.ai_provider, "lmstudio");
    }

    #[test]
    fn reset_paired_settings_returns_to_local_defaults() {
        let mut cfg = AppConfig::default();
        cfg.ai_provider = "ollama".into();
        apply_paired_settings(&mut cfg, "192.168.4.37", &ports(Some(1235)));
        assert_eq!(cfg.stt_mode, SttMode::Remote);

        reset_paired_settings(&mut cfg);

        assert_eq!(cfg.stt_mode, SttMode::Local);
        assert_eq!(cfg.stt_remote_host, "");
        assert_eq!(cfg.stt_remote_port, 8080);
        assert_eq!(cfg.ollama_host, "localhost");
        assert_eq!(cfg.ollama_port, 11434);
        assert_eq!(cfg.lmstudio_host, "localhost");
        assert_eq!(cfg.lmstudio_port, 1234);
        assert_eq!(cfg.omlx_host, "localhost");
        assert_eq!(cfg.omlx_port, 8000);
        assert_eq!(cfg.ai_provider, "ollama", "ai_provider must be preserved");
    }
}
