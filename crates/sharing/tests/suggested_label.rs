use medical_sharing::suggested_label::{sanitise, suggested_client_label};

#[test]
fn sanitise_strips_trailing_dot_local_dot() {
    assert_eq!(sanitise("cortex-mbp.local."), "cortex-mbp");
}

#[test]
fn sanitise_strips_trailing_dot_local() {
    assert_eq!(sanitise("cortex-mbp.local"), "cortex-mbp");
}

#[test]
fn sanitise_passes_through_plain_hostname() {
    assert_eq!(sanitise("clinic-front-desk"), "clinic-front-desk");
}

#[test]
fn sanitise_trims_whitespace() {
    assert_eq!(sanitise("  host  "), "host");
}

#[test]
fn sanitise_returns_laptop_for_empty_input() {
    assert_eq!(sanitise(""), "laptop");
    assert_eq!(sanitise("   "), "laptop");
    assert_eq!(sanitise(".local"), "laptop");
    assert_eq!(sanitise(".local."), "laptop");
}

#[test]
fn suggested_returns_non_empty_string() {
    let s = suggested_client_label();
    assert!(!s.is_empty());
    assert_ne!(s, "this laptop");
    assert!(!s.ends_with(".local"));
    assert!(!s.ends_with(".local."));
}
