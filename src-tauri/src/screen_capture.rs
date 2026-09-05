//! Interactive screen-region capture for the screenshot-OCR feature.
//!
//! Platform priority (mirrors Omarchy's `omarchy-capture-text`):
//!
//! | Platform | Mechanism | Disk |
//! |---|---|---|
//! | macOS | `screencapture -i -x <file>` (the one tool with no stdout mode) | private app dir, shredded before OCR |
//! | Linux (Wayland) | `slurp` (geometry) + `grim -g <geom> -` (PNG on stdout) | none |
//! | Linux (X11) / Windows | transparent fullscreen overlay + `xcap` screen API | none |
//!
//! **PHI constraint (load-bearing):** the captured pixels are patient data.
//! Bytes stay in memory except on macOS, where `screencapture` can only write
//! a file — there, and only there, the PNG lands in a private app-controlled
//! directory (mode 0700, file 0600), is read immediately, and is shredded +
//! unlinked BEFORE the OCR call runs. Never a shared `/tmp`; never logged.
//! Only the final OCR *text* is ever written to a clipboard.

use std::path::Path;

/// A rectangle in physical screen coordinates (pixels).
///
/// Compiled on every platform (and exercised by unit tests everywhere) but
/// only constructed by the Linux/Windows capture arms — hence the cfg.
#[cfg(any(not(target_os = "macos"), test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Error from region capture. `Cancelled` is the expected "user pressed Esc /
/// dragged nothing" outcome — callers surface it as a notice, not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegionCaptureError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for RegionCaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegionCaptureError::Cancelled => write!(f, "Region selection was cancelled"),
            RegionCaptureError::Failed(msg) => write!(f, "{msg}"),
        }
    }
}

/// Overall cap on any single interactive capture. A user walks away mid-select
/// or a platform tool hangs; without this the in-flight guard would block the
/// feature until app restart.
#[cfg_attr(target_os = "windows", allow(dead_code))]
const CAPTURE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(600);

// ---------------------------------------------------------------------------
// Pure geometry/format helpers (unit-tested on every platform)
// ---------------------------------------------------------------------------

/// Parse `slurp`'s geometry output: `"x,y WxH"` (e.g. `"10,20 300x400"`).
/// Also accepts `"x,y,W,H"` and ImageMagick-style `"WxH+X+Y"` for robustness
/// against alternative region printers.
#[cfg(any(target_os = "linux", test))]
pub fn parse_slurp_geometry(s: &str) -> Option<CaptureRect> {
    let s = s.trim();
    if let Some((pos, size)) = s.split_once(' ') {
        let (x, y) = pos.split_once(',')?;
        let (w, h) = size.split_once('x')?;
        return rect_from_parts(x, y, w, h);
    }
    if let Some((size, pos)) = s.split_once('+') {
        // ImageMagick style: WxH+X+Y
        let (w, h) = size.split_once('x')?;
        let (x, y) = pos.split_once('+')?;
        return rect_from_parts(x, y, w, h);
    }
    // Comma-separated x,y,w,h
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() == 4 {
        return rect_from_parts(parts[0], parts[1], parts[2], parts[3]);
    }
    None
}

/// Build a rect from string parts, rejecting negatives for width/height.
#[cfg(any(target_os = "linux", test))]
fn rect_from_parts(x: &str, y: &str, w: &str, h: &str) -> Option<CaptureRect> {
    let x: i32 = x.trim().parse().ok()?;
    let y: i32 = y.trim().parse().ok()?;
    let width: u32 = w.trim().parse().ok()?;
    let height: u32 = h.trim().parse().ok()?;
    // A zero-size selection is a cancel, not a capture.
    (width > 0 && height > 0).then_some(CaptureRect {
        x,
        y,
        width,
        height,
    })
}

/// Clamp `rect` to a `0..bounds_w` × `0..bounds_h` image, translating the
/// rect into image-local coordinates (subtracting nothing — the caller passes
/// image-relative input). Returns `None` when nothing of the rect survives
/// the intersection.
#[cfg(any(not(target_os = "macos"), test))]
pub fn clamp_rect(rect: CaptureRect, bounds_w: i32, bounds_h: i32) -> Option<CaptureRect> {
    let left = rect.x.max(0);
    let top = rect.y.max(0);
    // Edge math in i64: a huge u32 extent cast to i32 wraps NEGATIVE (e.g.
    // u32::MAX as i32 == -1), which would make the intersection look empty.
    let right = (rect.x as i64)
        .saturating_add(rect.width as i64)
        .min(bounds_w as i64);
    let bottom = (rect.y as i64)
        .saturating_add(rect.height as i64)
        .min(bounds_h as i64);
    let width = right.saturating_sub(left as i64);
    let height = bottom.saturating_sub(top as i64);
    (width > 0 && height > 0).then_some(CaptureRect {
        x: left,
        y: top,
        width: width as u32,
        height: height as u32,
    })
}

/// PNG signature check — validates that whatever the platform tool produced
/// is actually a PNG before it is handed to the OCR data-URL encoder.
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub fn is_png(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && bytes[..8] == [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
}

// ---------------------------------------------------------------------------
// Capture dispatch
// ---------------------------------------------------------------------------

/// Interactively capture a screen region as in-memory PNG bytes.
///
/// Blocks (asynchronously) until the user finishes or cancels the selection.
/// `data_dir` is the app data dir — used only by the macOS arm for its
/// private capture dir.
pub async fn capture_region_png(
    app: &tauri::AppHandle,
    data_dir: &Path,
) -> Result<Vec<u8>, RegionCaptureError> {
    // Only the X11/Windows overlay arm uses the app handle; silence the
    // unused binding on arms that don't.
    let _ = app;
    #[cfg(target_os = "macos")]
    {
        capture_macos(data_dir).await
    }
    #[cfg(target_os = "linux")]
    {
        if is_wayland() {
            capture_linux_wayland().await
        } else {
            capture_overlay_x11_windows(app).await
        }
    }
    #[cfg(target_os = "windows")]
    {
        capture_overlay_x11_windows(app).await
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (app, data_dir);
        Err(RegionCaptureError::Failed(
            "Screen capture is not supported on this platform".into(),
        ))
    }
}

/// True when the session is Wayland (compositor owns global hotkeys and only
/// `grim`-style wlr-screencopy / the portal can grab pixels).
#[cfg(target_os = "linux")]
fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland"))
}

// ---------------------------------------------------------------------------
// macOS: screencapture
// ---------------------------------------------------------------------------

/// Run the system interactive region picker and return the PNG bytes.
///
/// `screencapture` is the ONE capture tool in the matrix with no stdout
/// mode, so macOS is the one platform where pixels transit disk — under the
/// constraints documented on the module: private 0700 dir inside the app data
/// dir, file pre-created 0600, read immediately, then shredded and unlinked
/// before this function returns (i.e. before OCR runs).
#[cfg(target_os = "macos")]
async fn capture_macos(data_dir: &Path) -> Result<Vec<u8>, RegionCaptureError> {
    let dir = private_capture_dir(data_dir)?;
    // Random name; never logged. screencapture truncates and rewrites the
    // pre-created 0600 file (same inode, mode preserved).
    let path = dir.join(format!("capture-{}.png", uuid::Uuid::new_v4().simple()));

    // Pre-create with 0600 so the on-disk file never has group/other bits,
    // even briefly.
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| RegionCaptureError::Failed(format!("prepare capture file: {e}")))?;
    file.write_all(&[]).ok();

    let status = tokio::time::timeout(
        CAPTURE_DEADLINE,
        tokio::process::Command::new("screencapture")
            .arg("-i")
            .arg("-x")
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(),
    )
    .await
    .map_err(|_| RegionCaptureError::Failed("screencapture timed out".into()))?
    .map_err(|e| RegionCaptureError::Failed(format!("run screencapture: {e}")))?;

    // Read + destroy the file before OCR regardless of outcome below. Esc
    // (cancel) exits non-zero and usually leaves an empty/absent file.
    let bytes = tokio::task::spawn_blocking({
        let path = path.clone();
        move || {
            let bytes = std::fs::read(&path).unwrap_or_default();
            shred_and_unlink(&path);
            bytes
        }
    })
    .await
    .map_err(|e| RegionCaptureError::Failed(format!("capture read task failed: {e}")))?;

    if !status.success() || !is_png(&bytes) {
        // Cancel (Esc) or an unusable/empty capture — expected, not an error.
        tracing::debug!(
            ok = status.success(),
            bytes = bytes.len(),
            "region capture cancelled"
        );
        return Err(RegionCaptureError::Cancelled);
    }
    tracing::debug!(bytes = bytes.len(), "region captured");
    Ok(bytes)
}

/// The private directory macOS capture PNGs live in, created 0700.
#[cfg(target_os = "macos")]
fn private_capture_dir(data_dir: &Path) -> Result<std::path::PathBuf, RegionCaptureError> {
    use std::os::unix::fs::DirBuilderExt;
    let dir = data_dir.join("capture-tmp");
    if !dir.exists() {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&dir)
            .map_err(|e| RegionCaptureError::Failed(format!("create capture dir: {e}")))?;
    }
    Ok(dir)
}

/// Best-effort shred: overwrite the file's bytes with zeros, fsync, unlink.
/// APFS may satisfy the overwrite via copy-on-write clones, so this is
/// defense-in-depth, not a guarantee — the 0700 parent dir is the real
/// boundary. The unlink always runs, even when the overwrite fails.
#[cfg(target_os = "macos")]
fn shred_and_unlink(path: &Path) {
    use std::io::Write;
    let result = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|mut f| {
            let len = f.metadata()?.len() as usize;
            let zeros = vec![0u8; len.min(8 * 1024 * 1024)];
            let mut written = 0;
            while written < len {
                let n = len - written;
                let chunk = &zeros[..n.min(zeros.len())];
                f.write_all(chunk)?;
                written += chunk.len();
            }
            f.sync_all()
        });
    if let Err(e) = result {
        tracing::debug!(error = %e, "capture file shred best-effort step failed");
    }
    if let Err(e) = std::fs::remove_file(path) {
        tracing::debug!(error = %e, "capture file unlink failed");
    }
}

// ---------------------------------------------------------------------------
// Linux (Wayland): slurp + grim — the exact Omarchy path, no disk at all
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
async fn capture_linux_wayland() -> Result<Vec<u8>, RegionCaptureError> {
    // Region selection: slurp prints "x,y WxH"; Esc exits non-zero.
    let slurp = tokio::time::timeout(
        CAPTURE_DEADLINE,
        tokio::process::Command::new("slurp")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| RegionCaptureError::Failed("slurp timed out".into()))?
    .map_err(|e| {
        RegionCaptureError::Failed(format!(
            "slurp not available ({e}). Install slurp and grim for region OCR — Omarchy ships both."
        ))
    })?;

    let geometry = String::from_utf8_lossy(&slurp.stdout).trim().to_string();
    let rect = if !slurp.status.success() {
        None // user pressed Esc
    } else {
        parse_slurp_geometry(&geometry)
    };
    let rect = match rect {
        Some(r) => r,
        None => return Err(RegionCaptureError::Cancelled),
    };

    // Capture that region as PNG on stdout — pixels never touch disk.
    let grim = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::process::Command::new("grim")
            .arg("-g")
            .arg(format!(
                "{},{} {}x{}",
                rect.x, rect.y, rect.width, rect.height
            ))
            .arg("-")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output(),
    )
    .await
    .map_err(|_| RegionCaptureError::Failed("grim timed out".into()))?
    .map_err(|e| {
        RegionCaptureError::Failed(format!(
            "grim not available ({e}). Install slurp and grim for region OCR — Omarchy ships both."
        ))
    })?;

    if !grim.status.success() {
        return Err(RegionCaptureError::Failed(format!(
            "grim exited with {}",
            grim.status.code().unwrap_or(-1)
        )));
    }
    if !is_png(&grim.stdout) {
        return Err(RegionCaptureError::Cancelled);
    }
    tracing::debug!(bytes = grim.stdout.len(), "region captured (grim)");
    Ok(grim.stdout)
}

// ---------------------------------------------------------------------------
// X11 / Windows: transparent overlay + xcap screen API
// ---------------------------------------------------------------------------

/// The overlay page reports its drag rectangle in CSS pixels relative to the
/// overlay window; `None` means the user cancelled (Esc). Field reads happen
/// in the non-macOS overlay arm only.
#[cfg_attr(target_os = "macos", allow(dead_code))]
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct OverlayCssRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Managed state connecting the overlay window's `screen_region_submit`
/// invoke back to the awaiting `capture_overlay` future.
#[derive(Default)]
pub struct OverlaySelection {
    sender: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<Option<OverlayCssRect>>>>,
}

impl OverlaySelection {
    /// Resolve the pending capture with the overlay's answer. Returns false
    /// when no capture is pending (stale overlay double-submit).
    pub fn submit(&self, rect: Option<OverlayCssRect>) -> bool {
        if let Ok(mut guard) = self.sender.lock()
            && let Some(tx) = guard.take()
        {
            return tx.send(rect).is_ok();
        }
        false
    }

    /// Overlay-armed platforms only (X11/Windows).
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    fn install(&self, tx: tokio::sync::oneshot::Sender<Option<OverlayCssRect>>) {
        if let Ok(mut guard) = self.sender.lock() {
            *guard = Some(tx);
        }
    }
}

/// Overlay-armed platforms only (X11/Windows).
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub const OVERLAY_WINDOW_LABEL: &str = "screen-region-overlay";
/// URL fragment the SPA switches on to render ONLY the region-select overlay
/// (no app shell).
#[cfg_attr(target_os = "macos", allow(dead_code))]
const OVERLAY_URL_FRAGMENT: &str = "screen-region-overlay";
/// How long to wait for a rectangle before giving up and tearing the overlay
/// down (user walked away).
#[cfg_attr(target_os = "macos", allow(dead_code))]
const OVERLAY_SELECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// Grace between hiding the overlay and the screen capture so the compositor
/// has actually removed the overlay from the frame.
#[cfg_attr(target_os = "macos", allow(dead_code))]
const OVERLAY_HIDE_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

/// Overlay + platform screen API capture (Linux X11, Windows). PNG bytes are
/// produced entirely in memory.
#[cfg(not(target_os = "macos"))]
async fn capture_overlay_x11_windows(
    app: &tauri::AppHandle,
) -> Result<Vec<u8>, RegionCaptureError> {
    use tauri::Manager;

    let selection = app.state::<OverlaySelection>();
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<OverlayCssRect>>();
    selection.install(tx);

    // Compute the union of all monitors so the selection can happen on any
    // screen (physical px; x/y may be negative for monitors left of/above
    // the primary).
    let union = monitor_union().map_err(RegionCaptureError::Failed)?;

    let window = tauri::WebviewWindowBuilder::new(
        app,
        OVERLAY_WINDOW_LABEL,
        tauri::WebviewUrl::App(format!("index.html#{OVERLAY_URL_FRAGMENT}").into()),
    )
    .title("FerriScribe — select region")
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .shadow(false)
    .focused(true)
    .build()
    .map_err(|e| RegionCaptureError::Failed(format!("open selection overlay: {e}")))?;

    // Frameless window covering the whole virtual desktop.
    let _ = window.set_position(tauri::PhysicalPosition::new(union.x, union.y));
    let _ = window.set_size(tauri::PhysicalSize::new(union.width, union.height));

    // Esc via the overlay page lands in `screen_region_submit(None)`; a
    // force-close of the window itself resolves the channel here too.
    {
        let app = app.clone();
        window.on_window_event(move |event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                app.state::<OverlaySelection>().submit(None);
            }
        });
    }

    let css_rect = tokio::time::timeout(OVERLAY_SELECT_TIMEOUT, rx).await;
    let css_rect = match css_rect {
        Ok(Ok(rect)) => rect,
        Ok(Err(_)) => None, // submitter dropped without sending
        Err(_) => None,     // timed out
    };

    // Tear the overlay down (or at least hide it) before grabbing pixels so
    // the selection UI is not in the shot.
    let _ = window.hide();
    if css_rect.is_none() {
        let _ = window.destroy();
        return Err(RegionCaptureError::Cancelled);
    }
    let css_rect = css_rect.expect("checked is_none above");

    tokio::time::sleep(OVERLAY_HIDE_GRACE).await;

    // Map CSS px (window-relative) → physical screen px.
    let scale = window.scale_factor().max(0.01);
    let origin = window.inner_position().map_err(|e| {
        let _ = window.destroy();
        RegionCaptureError::Failed(format!("overlay position: {e}"))
    })?;
    let phys_rect = CaptureRect {
        x: origin.x + (css_rect.x * scale).round() as i32,
        y: origin.y + (css_rect.y * scale).round() as i32,
        width: (css_rect.width * scale).round().max(1.0) as u32,
        height: (css_rect.height * scale).round().max(1.0) as u32,
    };

    let png = tokio::task::spawn_blocking(move || capture_screen_region_png(phys_rect))
        .await
        .map_err(|e| RegionCaptureError::Failed(format!("capture task failed: {e}")));
    let _ = window.destroy();
    tracing::debug!(
        bytes = png.as_ref().map_or(0, Vec::len),
        "region captured (xcap)"
    );
    png
}

/// Bounding box of all monitors in physical pixels.
#[cfg(not(target_os = "macos"))]
fn monitor_union() -> Result<CaptureRect, String> {
    let monitors = xcap::Monitor::all().map_err(|e| format!("list monitors: {e}"))?;
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for m in &monitors {
        let x = m.x().map_err(|e| format!("monitor x: {e}"))?;
        let y = m.y().map_err(|e| format!("monitor y: {e}"))?;
        let w = m.width().map_err(|e| format!("monitor w: {e}"))? as i32;
        let h = m.height().map_err(|e| format!("monitor h: {e}"))? as i32;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x + w);
        max_y = max_y.max(y + h);
    }
    if min_x > max_x || min_y > max_y {
        return Err("no monitors found".into());
    }
    Ok(CaptureRect {
        x: min_x,
        y: min_y,
        width: (max_x - min_x) as u32,
        height: (max_y - min_y) as u32,
    })
}

/// Capture the monitor containing the rect's center, crop to the rect
/// (monitor-local, clamped), force the alpha channel opaque (GDI/DXGI frames
/// can carry a zero alpha that would render the PNG transparent), and encode
/// PNG — all in memory.
#[cfg(not(target_os = "macos"))]
fn capture_screen_region_png(rect: CaptureRect) -> Result<Vec<u8>, RegionCaptureError> {
    let cx = rect.x.saturating_add((rect.width / 2) as i32);
    let cy = rect.y.saturating_add((rect.height / 2) as i32);
    let monitor = xcap::Monitor::from_point(cx, cy)
        .map_err(|e| RegionCaptureError::Failed(format!("find monitor for selection: {e}")))?;
    let (mon_x, mon_y) = (
        monitor
            .x()
            .map_err(|e| RegionCaptureError::Failed(format!("monitor x: {e}")))?,
        monitor
            .y()
            .map_err(|e| RegionCaptureError::Failed(format!("monitor y: {e}")))?,
    );
    let image = monitor
        .capture_image()
        .map_err(|e| RegionCaptureError::Failed(format!("capture screen: {e}")))?;

    let local = CaptureRect {
        x: rect.x - mon_x,
        y: rect.y - mon_y,
        ..rect
    };
    let local = clamp_rect(local, image.width() as i32, image.height() as i32)
        .ok_or_else(|| RegionCaptureError::Cancelled)?;

    let mut cropped = image
        .crop_imm(local.x as u32, local.y as u32, local.width, local.height)
        .to_owned();
    for pixel in cropped.pixels_mut() {
        pixel.0[3] = 255;
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(cropped)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| RegionCaptureError::Failed(format!("png encode: {e}")))?;
    Ok(png)
}

/// Tauri command the overlay page invokes with its drag rectangle (CSS px,
/// window-relative). `None` = cancelled.
#[tauri::command]
pub fn screen_region_submit(
    app: tauri::AppHandle,
    rect: Option<OverlayCssRect>,
) -> Result<bool, String> {
    use tauri::Manager;
    Ok(app.state::<OverlaySelection>().submit(rect))
}

// ---------------------------------------------------------------------------
// Tests — pure geometry/format handling runs on every platform/CI leg.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_slurp_default_geometry() {
        assert_eq!(
            parse_slurp_geometry("10,20 300x400"),
            Some(CaptureRect {
                x: 10,
                y: 20,
                width: 300,
                height: 400
            })
        );
        // Tolerate surrounding whitespace/newline.
        assert_eq!(
            parse_slurp_geometry("  0,0 1920x1080\n"),
            Some(CaptureRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080
            })
        );
    }

    #[test]
    fn parses_alternate_geometry_formats() {
        assert_eq!(
            parse_slurp_geometry("300x400+10+20"),
            Some(CaptureRect {
                x: 10,
                y: 20,
                width: 300,
                height: 400
            })
        );
        assert_eq!(
            parse_slurp_geometry("10,20,300,400"),
            Some(CaptureRect {
                x: 10,
                y: 20,
                width: 300,
                height: 400
            })
        );
    }

    #[test]
    fn rejects_malformed_or_degenerate_geometry() {
        // Empty output = user cancelled / garbage.
        for bad in [
            "",
            "   ",
            "10,20",
            "a,b 1x1",
            "10,20 0x400",
            "10,20 300x0",
            "10,20 -3x4",
        ] {
            assert_eq!(parse_slurp_geometry(bad), None, "should reject {bad:?}");
        }
    }

    #[test]
    fn accepts_negative_origins() {
        // Monitors arranged left of the primary have negative coordinates.
        assert_eq!(
            parse_slurp_geometry("-1920,0 1920x1080"),
            Some(CaptureRect {
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080
            })
        );
    }

    #[test]
    fn clamp_rect_intersects_and_translates() {
        let full = CaptureRect {
            x: 10,
            y: 10,
            width: 100,
            height: 100,
        };
        // Fully inside → unchanged.
        assert_eq!(
            clamp_rect(full, 1920, 1080),
            Some(CaptureRect {
                x: 10,
                y: 10,
                width: 100,
                height: 100
            })
        );
        // Hanging off the right/bottom edge → truncated.
        assert_eq!(
            clamp_rect(
                CaptureRect {
                    x: 1900,
                    y: 1000,
                    width: 100,
                    height: 100
                },
                1920,
                1080
            ),
            Some(CaptureRect {
                x: 1900,
                y: 1000,
                width: 20,
                height: 80
            })
        );
        // Negative origin → clamped to 0 with size reduced accordingly.
        assert_eq!(
            clamp_rect(
                CaptureRect {
                    x: -10,
                    y: -10,
                    width: 100,
                    height: 100
                },
                1920,
                1080
            ),
            Some(CaptureRect {
                x: 0,
                y: 0,
                width: 90,
                height: 90
            })
        );
        // Completely outside → None (nothing to OCR).
        assert_eq!(
            clamp_rect(
                CaptureRect {
                    x: 2000,
                    y: 0,
                    width: 10,
                    height: 10
                },
                1920,
                1080
            ),
            None
        );
        // Zero-size after clamp → None.
        assert_eq!(
            clamp_rect(
                CaptureRect {
                    x: 1920,
                    y: 0,
                    width: 10,
                    height: 10
                },
                1920,
                1080
            ),
            None
        );
    }

    #[test]
    fn clamp_rect_saturates_on_huge_values() {
        // i32::MAX width must not overflow when added to x.
        let rect = CaptureRect {
            x: 0,
            y: 0,
            width: u32::MAX,
            height: u32::MAX,
        };
        assert_eq!(
            clamp_rect(rect, 100, 100),
            Some(CaptureRect {
                x: 0,
                y: 0,
                width: 100,
                height: 100
            })
        );
    }

    #[test]
    fn sniffs_png_signature() {
        assert!(is_png(&[
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0
        ]));
        assert!(!is_png(b"PNGPNGPNG"));
        assert!(!is_png(&[0x89, b'P', b'N', b'G']));
        assert!(!is_png(b""));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn private_capture_dir_is_created_0700() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = private_capture_dir(tmp.path()).unwrap();
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "capture dir must be owner-only");
        // Idempotent: second call returns the same dir.
        assert_eq!(private_capture_dir(tmp.path()).unwrap(), dir);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn shred_and_unlink_removes_file_contents_and_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("capture-test.png");
        std::fs::write(&path, vec![0xAB_u8; 4096]).unwrap();
        shred_and_unlink(&path);
        assert!(!path.exists(), "capture file must be unlinked");
    }
}
