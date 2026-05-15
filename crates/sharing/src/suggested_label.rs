//! Sanitised OS hostname used as the default label a client sends to the
//! office server at pair time. Never PHI — the OS hostname is set by the
//! machine owner and is not patient data.

/// Trim whitespace, strip a trailing `.local.` then `.local`, and fall
/// back to `"laptop"` if nothing useful is left. Pure / synchronous so
/// the unit tests target it directly.
pub fn sanitise(raw: &str) -> String {
    let mut s = raw.trim();
    if let Some(stripped) = s.strip_suffix(".local.") {
        s = stripped;
    } else if let Some(stripped) = s.strip_suffix(".local") {
        s = stripped;
    }
    let s = s.trim();
    if s.is_empty() {
        "laptop".to_string()
    } else {
        s.to_string()
    }
}

/// Best-effort hostname-derived default label. Falls back to `"laptop"`
/// if the OS hostname lookup fails or is unusable.
pub fn suggested_client_label() -> String {
    match hostname::get() {
        Ok(os) => sanitise(&os.to_string_lossy()),
        Err(_) => "laptop".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitise_passes_through_plain_ascii() {
        assert_eq!(sanitise("clinic-laptop"), "clinic-laptop");
        assert_eq!(sanitise("workstation42"), "workstation42");
    }

    #[test]
    fn sanitise_trims_surrounding_whitespace() {
        assert_eq!(sanitise("  host  "), "host");
        assert_eq!(sanitise("\t\nhost\n"), "host");
    }

    #[test]
    fn sanitise_strips_dot_local_suffix() {
        assert_eq!(sanitise("clinic.local"), "clinic");
    }

    #[test]
    fn sanitise_strips_dot_local_dot_suffix() {
        // ".local." is mDNS-style and takes priority over ".local".
        assert_eq!(sanitise("clinic.local."), "clinic");
    }

    #[test]
    fn sanitise_falls_back_to_laptop_for_empty_or_whitespace() {
        assert_eq!(sanitise(""), "laptop");
        assert_eq!(sanitise("   "), "laptop");
    }

    #[test]
    fn sanitise_falls_back_to_laptop_for_lone_dot_local() {
        // ".local" with nothing in front should collapse to fallback.
        assert_eq!(sanitise(".local"), "laptop");
        assert_eq!(sanitise(".local."), "laptop");
    }

    #[test]
    fn suggested_client_label_returns_non_empty_string() {
        // We can't predict the host's actual hostname, but the function
        // must always return at least one character (either real or "laptop").
        let label = suggested_client_label();
        assert!(!label.is_empty(), "label must never be empty; got {label:?}");
    }
}
