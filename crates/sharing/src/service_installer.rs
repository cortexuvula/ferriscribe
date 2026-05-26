//! Service installer — per-platform persistent Ollama service writers (launchd / systemd / scheduled task).
//!
//! Writes a service unit that runs `ollama serve` with
//! OLLAMA_HOST=127.0.0.1:11434 so Ollama remains loopback-only. The auth
//! proxy is what's exposed on the network.

use std::path::PathBuf;

use crate::SharingError;

/// Find the absolute path to the `ollama` binary. Tries `which ollama`
/// first (or `where.exe ollama` on Windows), then falls back to a list of
/// known install locations per platform. Returns `None` if not found.
fn find_ollama_binary() -> Option<PathBuf> {
    // 1. Try `which` / `where.exe`.
    #[cfg(unix)]
    let probe = std::process::Command::new("which").arg("ollama").output().ok();
    #[cfg(windows)]
    let probe = std::process::Command::new("where.exe").arg("ollama").output().ok();

    if let Some(out) = probe {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            // `where.exe` may return multiple lines; take the first.
            let first = s.lines().next().unwrap_or("").to_string();
            if !first.is_empty() {
                let p = PathBuf::from(first);
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }

    // 2. Per-platform known locations.
    #[cfg(target_os = "macos")]
    let candidates: &[&str] = &[
        "/opt/homebrew/bin/ollama",   // Apple Silicon Homebrew
        "/usr/local/bin/ollama",      // Intel Homebrew / official installer
        "/usr/bin/ollama",
    ];
    #[cfg(target_os = "linux")]
    let candidates: &[&str] = &[
        "/usr/local/bin/ollama",
        "/usr/bin/ollama",
        "/snap/bin/ollama",
    ];
    #[cfg(target_os = "windows")]
    let candidates: &[&str] = &[
        r"C:\Program Files\Ollama\ollama.exe",
    ];
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let candidates: &[&str] = &[];

    for c in candidates {
        let p = PathBuf::from(c);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

/// Escape characters that are special in XML attribute values and text content.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Returns true if something is already listening on Ollama's default port.
/// Used to avoid installing a competing persistent service when the user
/// already has Ollama managed externally (homebrew, the Ollama.app installer,
/// a manual `ollama serve`, etc.). A bound port means our `ollama serve`
/// would fail to bind anyway, so skipping is the right default.
fn ollama_port_in_use() -> bool {
    use std::net::{SocketAddr, TcpStream};
    use std::time::Duration;
    let addr: SocketAddr = match "127.0.0.1:11434".parse() {
        Ok(a) => a,
        Err(_) => return false,
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Installed,
    Missing,
    UnknownPlatform,
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    fn plist_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/LaunchAgents/com.ferriscribe.ollama.plist")
    }

    pub fn install() -> Result<(), SharingError> {
        if super::ollama_port_in_use() {
            tracing::info!(
                "skipping persistent ollama install: port 11434 already bound (managed externally)"
            );
            return Ok(());
        }
        let path = plist_path();
        let bin = super::find_ollama_binary().ok_or_else(|| {
            SharingError::ServiceInstaller(
                "Ollama binary not found. Install Ollama first (https://ollama.com/download)."
                    .to_string(),
            )
        })?;
        let bin_str = super::xml_escape(&bin.to_string_lossy());
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.ferriscribe.ollama</string>
  <key>ProgramArguments</key>
  <array><string>{bin_str}</string><string>serve</string></array>
  <key>EnvironmentVariables</key>
  <dict><key>OLLAMA_HOST</key><string>127.0.0.1:11434</string></dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
"#
        );
        let parent = path.parent()
            .ok_or_else(|| SharingError::InvalidPath(format!("no parent dir: {}", path.display())))?;
        std::fs::create_dir_all(parent)
            .map_err(SharingError::Io)?;
        std::fs::write(&path, plist).map_err(SharingError::Io)?;
        let status = std::process::Command::new("launchctl")
            .args(["load", "-w"])
            .arg(&path)
            .status()
            .map_err(SharingError::Io)?;
        if !status.success() {
            return Err(SharingError::ServiceInstaller(format!(
                "launchctl load exited with {}", status
            )));
        }
        tracing::info!("persistent ollama LaunchAgent installed and loaded");
        Ok(())
    }

    pub fn state() -> ServiceState {
        if plist_path().exists() {
            ServiceState::Installed
        } else {
            ServiceState::Missing
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    fn unit_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("systemd/user/ferriscribe-ollama.service")
    }

    pub fn install() -> Result<(), SharingError> {
        if super::ollama_port_in_use() {
            tracing::info!(
                "skipping persistent ollama install: port 11434 already bound (managed externally)"
            );
            return Ok(());
        }
        let path = unit_path();
        let bin = super::find_ollama_binary().ok_or_else(|| {
            SharingError::ServiceInstaller(
                "Ollama binary not found. Install Ollama first (https://ollama.com/download)."
                    .to_string(),
            )
        })?;
        let bin_str = bin.to_string_lossy();
        let unit = format!(
            "[Unit]\nDescription=Ollama (managed by FerriScribe)\n\n[Service]\nEnvironment=OLLAMA_HOST=127.0.0.1:11434\nExecStart={bin_str} serve\nRestart=on-failure\n\n[Install]\nWantedBy=default.target\n"
        );
        let parent = path.parent()
            .ok_or_else(|| SharingError::InvalidPath(format!("no parent dir: {}", path.display())))?;
        std::fs::create_dir_all(parent)
            .map_err(SharingError::Io)?;
        std::fs::write(&path, unit).map_err(SharingError::Io)?;
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "enable", "--now", "ferriscribe-ollama.service"])
            .status();
        tracing::info!("persistent ollama systemd unit installed and enabled");
        Ok(())
    }

    pub fn state() -> ServiceState {
        if unit_path().exists() {
            ServiceState::Installed
        } else {
            ServiceState::Missing
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;

    pub fn install() -> Result<(), SharingError> {
        if super::ollama_port_in_use() {
            tracing::info!(
                "skipping persistent ollama install: port 11434 already bound (managed externally)"
            );
            return Ok(());
        }
        let bin = super::find_ollama_binary().ok_or_else(|| {
            SharingError::ServiceInstaller(
                "Ollama binary not found. Install Ollama first (https://ollama.com/download)."
                    .to_string(),
            )
        })?;
        // Escape XML-special characters in the path before embedding it in the
        // XML document. Then wrap in &quot; so cmd.exe treats it as a quoted
        // argument — without this, a path like C:\Program Files\… causes
        // cmd.exe to execute "C:\Program" with the rest as arguments.
        let bin_str = super::xml_escape(&bin.to_string_lossy());
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers>
  <Actions Context="Author">
    <Exec>
      <Command>cmd.exe</Command>
      <Arguments>/c set OLLAMA_HOST=127.0.0.1:11434 &amp; &quot;{bin_str}&quot; serve</Arguments>
    </Exec>
  </Actions>
</Task>"#
        );
        let dir = std::env::temp_dir();
        let xml_path = dir.join("ferriscribe-ollama.xml");
        // Prepend a UTF-8 BOM so older Windows builds (pre-1809) parse the
        // file encoding correctly.
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(xml.as_bytes());
        std::fs::write(&xml_path, bytes).map_err(SharingError::Io)?;
        let status = std::process::Command::new("schtasks")
            .args(["/Create", "/TN", "FerriScribe Ollama", "/XML"])
            .arg(&xml_path)
            .args(["/F"])
            .status()
            .map_err(SharingError::Io)?;
        if !status.success() {
            return Err(SharingError::ServiceInstaller(format!(
                "schtasks /Create exited with {}", status
            )));
        }
        tracing::info!("persistent ollama scheduled task installed");
        Ok(())
    }

    pub fn state() -> ServiceState {
        let out = std::process::Command::new("schtasks")
            .args(["/Query", "/TN", "FerriScribe Ollama"])
            .output();
        match out {
            Ok(o) if o.status.success() => ServiceState::Installed,
            Ok(_) => ServiceState::Missing,
            Err(_) => ServiceState::UnknownPlatform,
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos::{install as install_persistent_ollama, state as ollama_service_state};
#[cfg(target_os = "linux")]
pub use linux::{install as install_persistent_ollama, state as ollama_service_state};
#[cfg(target_os = "windows")]
pub use windows::{install as install_persistent_ollama, state as ollama_service_state};

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn install_persistent_ollama() -> Result<(), SharingError> {
    Err(SharingError::ServiceInstaller("unsupported platform".into()))
}
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn ollama_service_state() -> ServiceState { ServiceState::UnknownPlatform }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_escape_replaces_special_chars() {
        assert_eq!(
            xml_escape(r#"a & b < c > d "e""#),
            "a &amp; b &lt; c &gt; d &quot;e&quot;"
        );
        assert_eq!(xml_escape("safe-string_123"), "safe-string_123");
    }

    #[test]
    fn xml_escape_empty_input_is_empty() {
        assert_eq!(xml_escape(""), "");
    }

    #[test]
    fn xml_escape_handles_ampersand_before_other_chars() {
        // Input literally contains "&lt;" — we must NOT see "&amp;amp;lt;"
        // (which would happen if we replaced < before & on this input).
        // The chain replaces & first, so "&lt;" becomes "&amp;lt;".
        assert_eq!(xml_escape("&lt;"), "&amp;lt;");
        assert_eq!(xml_escape("&&"), "&amp;&amp;");
    }

    #[test]
    fn xml_escape_handles_realistic_windows_path() {
        // Defends the Windows ScheduledTask install() path against
        // unescaped '&' injection in folder names.
        let input = r"C:\Program Files & Co\ollama.exe";
        let expected = r"C:\Program Files &amp; Co\ollama.exe";
        assert_eq!(xml_escape(input), expected);
    }

    /// Resolves the parent directory of `path`, returning
    /// `SharingError::InvalidPath` when `Path::parent()` yields `None`
    /// (e.g. for the filesystem root "/").  Mirrors the exact pattern used
    /// inside `macos::install()` and `linux::install()`.
    fn resolve_parent(path: &std::path::Path) -> Result<&std::path::Path, SharingError> {
        path.parent()
            .ok_or_else(|| SharingError::InvalidPath(format!("no parent dir: {}", path.display())))
    }

    #[test]
    fn path_parent_none_propagates_invalid_path_error() {
        use std::path::Path;
        // The filesystem root "/" has no parent.
        let root = Path::new("/");
        let result = resolve_parent(root);
        assert!(result.is_err(), "expected Err for root path, got: {:?}", result);
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("no parent dir"),
            "expected reason in error message, got: {msg}"
        );
        assert!(
            msg.contains('/'),
            "expected offending path in error message, got: {msg}"
        );
    }

    #[test]
    fn path_parent_some_returns_parent_for_normal_path() {
        use std::path::Path;
        let path = Path::new("/Users/me/Library/LaunchAgents/com.ferriscribe.ollama.plist");
        let parent = resolve_parent(path).expect("normal path must have a parent");
        assert_eq!(parent, Path::new("/Users/me/Library/LaunchAgents"));
    }
}
