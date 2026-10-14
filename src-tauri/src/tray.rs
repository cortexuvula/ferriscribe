//! Menu-bar / system-tray recording indicator (`tray-recording-indicator`).
//!
//! An always-best-effort tray: an icon that is a GREEN FILLED DISC while
//! idle and a RED HOLLOW RING while a recording session is active (the two
//! states differ by SHAPE as well as colour — red/green is the classic
//! colour-blind confusion pair), with a minimal menu: a disabled status
//! line, a separator, "Show FerriScribe", and "Quit FerriScribe".
//!
//! # Hard rules encoded here
//!
//! - **Never fatal.** Release builds use `panic = "abort"` and `lib.rs`
//!   `.expect`s on run, so any error propagated out of setup — or any
//!   panic — kills the app at boot. Every failure path below logs the
//!   error KIND (a static per-step message; the error payload itself is
//!   never logged) and returns.
//! - **PHI-free by construction.** The only strings in the tray surface
//!   are the static constants below. No recording ids, filenames,
//!   timestamps, durations, clinical text, or error message content ever
//!   reach the tray, tooltip, menu, or log lines from this module.
//! - **No menu-event listener here.** `tray-show` / `tray-quit` route
//!   through the app's single global `on_menu_event` match in `lib.rs`
//!   (same listener as the app menu). Registering a builder-scoped menu
//!   listener would double-fire menu events.
//! - **Linux availability guard.** `libappindicator-sys` PANICS inside
//!   its loader when the AppIndicator runtime library is absent — fatal
//!   under `panic = "abort"` — so [`init_tray`] dlopen-probes for the
//!   library first and skips the tray entirely (one warn) when none
//!   loads. On GNOME without the AppIndicator extension the tray simply
//!   won't show; that is acceptable and does not crash.
//!
//! Wiring (built in the existing setup closure, never via a `trayIcon`
//! entry in `tauri.conf.json` — config-defined trays appear in helper
//! processes too): see the batch-2 setup hook in `lib.rs`.

use std::sync::OnceLock;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;

/// Tray id. Distinct from every app-menu item id, so the global
/// `on_menu_event` match can route tray clicks unambiguously.
const TRAY_ID: &str = "main";

/// Disabled status-line menu item — the Linux tooltip fallback (Linux
/// trays have no tooltips) and an accessibility signal everywhere.
const STATUS_ITEM_ID: &str = "tray-status";

/// "Show FerriScribe" — re-surfaces the main window. Matched in `lib.rs`'s
/// global menu-event handler (batch 2).
pub(crate) const TRAY_SHOW_ID: &str = "tray-show";

/// "Quit FerriScribe" — routes to the coordinated quit (refuses while a
/// recording is active). Matched in `lib.rs`'s global menu-event handler
/// (batch 2).
pub(crate) const TRAY_QUIT_ID: &str = "tray-quit";

const SHOW_LABEL: &str = "Show FerriScribe";
const QUIT_LABEL: &str = "Quit FerriScribe";

const TOOLTIP_IDLE: &str = "FerriScribe - Not recording";
const TOOLTIP_RECORDING: &str = "FerriScribe - Recording";
const STATUS_TEXT_IDLE: &str = "Not recording";
const STATUS_TEXT_RECORDING: &str = "Recording";

// Icons are decoded to raw RGBA at COMPILE time (tauri-macros) — no
// runtime decode step, no `image-png` cargo feature, and no failure path
// at init. Paths resolve from `src-tauri/` (CARGO_MANIFEST_DIR).
const IDLE_ICON: tauri::image::Image<'static> = tauri::include_image!("icons/tray-idle.png");
const RECORDING_ICON: tauri::image::Image<'static> =
    tauri::include_image!("icons/tray-recording.png");

/// The complete tray presentation for one recording state — the single
/// pure mapping that init and live updates both consume (pinned by the
/// tests below).
struct TrayView {
    icon: tauri::image::Image<'static>,
    tooltip: &'static str,
    status_text: &'static str,
}

fn tray_view(active: bool) -> TrayView {
    if active {
        TrayView {
            icon: RECORDING_ICON.clone(),
            tooltip: TOOLTIP_RECORDING,
            status_text: STATUS_TEXT_RECORDING,
        }
    } else {
        TrayView {
            icon: IDLE_ICON.clone(),
            tooltip: TOOLTIP_IDLE,
            status_text: STATUS_TEXT_IDLE,
        }
    }
}

/// The watcher's change-detection decision, extracted pure: apply a tray
/// update only when the freshly-read flag differs from the last state the
/// tray showed. `None` (nothing applied yet) always updates, so the first
/// poll after boot initialises the tray and every steady-state poll is a
/// no-op — the watcher task in `lib.rs` calls this inside its loop.
pub(crate) fn should_update(last: Option<bool>, current: bool) -> bool {
    last != Some(current)
}

/// The disabled status-line item from the built tray. The tray icon itself
/// is reachable again via `app.tray_by_id(TRAY_ID)`, but a `MenuItem`
/// handle has no lookup path from the app — so the built item is parked
/// here once for [`set_recording_state`]. `MenuItem` is `Send + Sync`
/// (tauri marks it so), making the static sound.
static STATUS_ITEM: OnceLock<tauri::menu::MenuItem<tauri::Wry>> = OnceLock::new();

/// Build the tray with its menu and the idle icon. Best-effort: every
/// failure logs the failing step and returns — a tray problem must never
/// stop app boot. Intended to run once, from the setup closure (main
/// thread); a second call in one process warns and keeps the first tray.
pub fn init_tray(app: &tauri::AppHandle) {
    if !appindicator_runtime_available() {
        tracing::warn!("tray unavailable (appindicator runtime not found)");
        return;
    }

    let idle = tray_view(false);

    let status_item = match MenuItem::with_id(
        app,
        STATUS_ITEM_ID,
        idle.status_text,
        // Disabled: the status line is a label, not an action.
        false,
        None::<&str>,
    ) {
        Ok(item) => item,
        Err(_) => {
            tracing::warn!("tray init failed: status menu item");
            return;
        }
    };
    let separator = match PredefinedMenuItem::separator(app) {
        Ok(item) => item,
        Err(_) => {
            tracing::warn!("tray init failed: separator");
            return;
        }
    };
    let show_item = match MenuItem::with_id(app, TRAY_SHOW_ID, SHOW_LABEL, true, None::<&str>) {
        Ok(item) => item,
        Err(_) => {
            tracing::warn!("tray init failed: show menu item");
            return;
        }
    };
    let quit_item = match MenuItem::with_id(app, TRAY_QUIT_ID, QUIT_LABEL, true, None::<&str>) {
        Ok(item) => item,
        Err(_) => {
            tracing::warn!("tray init failed: quit menu item");
            return;
        }
    };
    let menu = match Menu::with_items(app, &[&status_item, &separator, &show_item, &quit_item]) {
        Ok(menu) => menu,
        Err(_) => {
            tracing::warn!("tray init failed: menu assembly");
            return;
        }
    };

    // The built tray stays registered in the app (retrieved later via
    // `app.tray_by_id(TRAY_ID)`); the returned handle itself is not kept.
    let built = TrayIconBuilder::with_id(TRAY_ID)
        // Non-template: colour IS the signal (green idle / red recording);
        // template icons are forced monochrome by the OS.
        .icon_as_template(false)
        .icon(idle.icon)
        // Tooltip is ignored on Linux — the disabled status menu line is
        // the status surface there.
        .tooltip(idle.tooltip)
        .menu(&menu)
        .build(app);
    if built.is_err() {
        tracing::warn!("tray init failed: tray build");
        return;
    }

    if STATUS_ITEM.set(status_item).is_err() {
        tracing::warn!("tray init skipped: already initialized");
    }
}

/// Swap the tray to the given recording state: icon, tooltip, and the
/// status menu line's text. No-op when the tray was never built (Linux
/// runtime library absent, or init failed) — a missing indicator is
/// never a reason to fail a recording action.
///
/// Callable from any thread (each setter hops to the main thread
/// internally, blocking until it runs) — callers must NOT hold locks the
/// main thread also takes; read the flag, drop the guard, then call.
pub fn set_recording_state(app: &tauri::AppHandle, active: bool) {
    // `tray_by_id` is an inherent AppHandle method (desktop + tray-icon
    // feature); it returns None when the tray was never built.
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let Some(status_item) = STATUS_ITEM.get() else {
        return;
    };
    let view = tray_view(active);

    if tray.set_icon(Some(view.icon)).is_err() {
        tracing::warn!("tray icon update failed");
    }
    if tray.set_tooltip(Some(view.tooltip)).is_err() {
        tracing::warn!("tray tooltip update failed");
    }
    if status_item.set_text(view.status_text).is_err() {
        tracing::warn!("tray status text update failed");
    }
}

/// Probe for the AppIndicator runtime library before any tray code runs.
/// The sonames `libappindicator-sys` itself probes, most-preferred first
/// (ayatana is the maintained fork). Any successful `dlopen` proves the
/// library is loadable; the handle is closed immediately — the sys crate
/// performs its own loading afterwards.
#[cfg(target_os = "linux")]
fn appindicator_runtime_available() -> bool {
    use std::ffi::CString;

    const CANDIDATES: &[&str] = &[
        "libayatana-appindicator3.so.1",
        "libappindicator3.so.1",
        "libayatana-appindicator3.so",
        "libappindicator3.so",
    ];
    for name in CANDIDATES {
        let Ok(cname) = CString::new(*name) else {
            continue;
        };
        // SAFETY: `cname` is a valid NUL-terminated byte string for the
        // whole call. The returned handle is passed straight to dlclose,
        // exactly once, on the same thread. RTLD_LAZY avoids resolving
        // the library's symbols now.
        let handle = unsafe { libc::dlopen(cname.as_ptr(), libc::RTLD_LAZY) };
        if !handle.is_null() {
            // SAFETY: handle is a live dlopen return value, used once.
            unsafe { libc::dlclose(handle) };
            return true;
        }
    }
    false
}

/// No runtime-library requirement on macOS / Windows — the guard is a
/// no-op there.
#[cfg(not(target_os = "linux"))]
fn appindicator_runtime_available() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The state→presentation mapping for both states (argus A6): the
    /// exact tooltip/status strings the tray surface shows.
    #[test]
    fn tray_view_maps_both_states() {
        let idle = tray_view(false);
        assert_eq!(idle.tooltip, "FerriScribe - Not recording");
        assert_eq!(idle.status_text, "Not recording");

        let recording = tray_view(true);
        assert_eq!(recording.tooltip, "FerriScribe - Recording");
        assert_eq!(recording.status_text, "Recording");
    }

    /// The two states carry different pixel data: the compile-time
    /// embedded icons decoded (not empty) and differ from each other —
    /// the shape+colour distinction in one assertion.
    #[test]
    fn tray_view_states_carry_distinct_embedded_icons() {
        let idle = tray_view(false);
        let recording = tray_view(true);

        assert_eq!(idle.icon.width(), 64);
        assert_eq!(idle.icon.height(), 64);
        assert_eq!(recording.icon.width(), 64);
        assert!(!idle.icon.rgba().is_empty());
        assert_ne!(idle.icon.rgba(), recording.icon.rgba());
    }

    /// Pixel (x, y) of an embedded icon as RGBA — `Image::rgba` is
    /// row-major RGBA8.
    fn pixel(view: &TrayView, x: u32, y: u32) -> [u8; 4] {
        let w = view.icon.width() as usize;
        let rgba = view.icon.rgba();
        let i = (y as usize * w + x as usize) * 4;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    /// Beyond differing bytes, the colour CONTRACT is pinned from the
    /// embedded pixel data: idle is a filled GREEN disc (opaque centre,
    /// green channel dominant), recording is a HOLLOW RED ring
    /// (transparent centre — the shape distinction, so a colour-blind
    /// user still sees a change — and an opaque red stroke). Replacing an
    /// asset with a same-shape recolour, or swapping the two files, fails
    /// here.
    #[test]
    fn tray_view_pins_colour_and_shape_per_state() {
        let idle = tray_view(false);
        let recording = tray_view(true);
        let (w, h) = (idle.icon.width(), idle.icon.height());
        let (mx, my) = (w / 2, h / 2);

        let [r, g, b, a] = pixel(&idle, mx, my);
        assert_eq!(a, u8::MAX, "idle disc centre must be opaque");
        assert!(g > r && g > b, "idle must be green, got r={r} g={g} b={b}");

        let [.., a] = pixel(&recording, mx, my);
        assert_eq!(a, 0, "recording ring centre must be transparent (hollow)");

        let [r, g, b, a] = pixel(&recording, w / 4, my);
        assert_eq!(a, u8::MAX, "recording ring stroke must be opaque");
        assert!(
            r > g && r > b,
            "recording must be red, got r={r} g={g} b={b}"
        );
    }

    /// The watcher's update decision: the first observation (`None`)
    /// always applies, and after that only real 0→1 / 1→0 transitions —
    /// steady-state polls never touch the tray.
    #[test]
    fn should_update_emits_only_on_change() {
        assert!(should_update(None, false), "None -> false: initial apply");
        assert!(should_update(None, true), "None -> true: initial apply");
        assert!(
            !should_update(Some(false), false),
            "false -> false: no update"
        );
        assert!(should_update(Some(false), true), "false -> true: update");
        assert!(!should_update(Some(true), true), "true -> true: no update");
        assert!(should_update(Some(true), false), "true -> false: update");
    }

    /// Structural pin in the translation.rs / restart.rs style: the tray
    /// is BUILT IN CODE by [`init_tray`] from the setup closure, never
    /// declared in `tauri.conf.json` — a config-defined `trayIcon` would
    /// also appear in the helper processes (`--capture-ocr` delegate, pill
    /// self-test), which reuse `app_context()` but must stay tray-less.
    #[test]
    fn tauri_conf_declares_no_tray_icon() {
        let conf = include_str!("../tauri.conf.json");
        assert!(
            !conf.contains("trayIcon"),
            "tauri.conf.json must not define trayIcon — the tray is code-built in setup, \
             helper processes must not get one"
        );
    }
}
