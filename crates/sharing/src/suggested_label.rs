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
