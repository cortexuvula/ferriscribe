//! Key-escrow artifacts: printable recovery sheet + offline USB file.
//!
//! Both artifacts carry the full wrapping key (each must be *independently
//! sufficient* for a clean-machine restore — the restore drill tests
//! recovery from EACH, not from both together) plus the escrow canary tag
//! so either can be verified with zero other secrets.

use std::path::Path;

use crate::BackupResult;
use crate::keys;

/// Suggested filename for the printed recovery sheet.
pub const SHEET_FILENAME: &str = "ferriscribe-backup-recovery-sheet.txt";
/// Suggested filename for the offline USB escrow file.
pub const USB_FILENAME: &str = "ferriscribe-backup-key.escrow";
/// Magic prefix of the binary USB artifact.
const USB_MAGIC: &[u8; 4] = b"FBK1";

const SHEET_BEGIN: &str = "-----BEGIN RECOVERY KEY-----";
const SHEET_END: &str = "-----END RECOVERY KEY-----";

/// Create (or truncate) `path` restricted to the owning user.
///
/// Called before any key material is written: both artifacts carry the
/// full wrapping key, so the default umask's group/other read bits (0644)
/// must never apply to them — especially since the suggested output
/// folder is `~/Desktop`, which sync/backup services routinely scoop.
fn create_private_file(path: &Path) -> std::io::Result<std::fs::File> {
    let file = std::fs::File::create(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // set_permissions rather than OpenOptions::mode(0o600): mode() only
        // applies at creation and is masked by the umask, and a pre-existing
        // file keeps its old permissions. An explicit chmod covers both,
        // before the payload is written.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        // Windows: new files inherit the containing directory's ACL; the
        // user-profile directories this tool writes into are already
        // user-scoped.
    }
    Ok(file)
}

/// Render the printable recovery sheet and write it to `path`.
///
/// Format: 8 lines of two 4-hex groups (64 hex chars total) between
/// marker lines, followed by a verification tag (canary prefix). The
/// key never appears anywhere else in the sheet, so the parser can
/// unambiguously reconstruct it.
pub fn write_recovery_sheet(path: &Path, wrapping: &[u8; 32]) -> std::io::Result<()> {
    use std::io::Write;
    let hex_key = hex::encode(wrapping);
    let canary = hex::encode(keys::escrow_canary_tag(wrapping));
    let mut body = String::new();
    for chunk in hex_key.as_bytes().chunks(8) {
        let s = std::str::from_utf8(chunk).expect("hex is ASCII");
        body.push_str(&format!("{} {}\n", &s[..4], &s[4..]));
    }
    let text = format!(
        "FerriScribe Backup Recovery Sheet\n\
         =================================\n\
         \n\
         Keep this sheet in a SAFE, off-machine, fire-protected place.\n\
         Together with your backup target it can restore ALL clinical data.\n\
         Anyone holding this sheet AND access to the backup target can read\n\
         your data — treat it like a master password.\n\
         \n\
         {SHEET_BEGIN}\n\
         {body}\
         {SHEET_END}\n\
         \n\
         Verification tag (first 16 hex of the check code):\n\
         {canary_prefix}\n\
         Full check code:\n\
         {canary}\n\
         \n\
         To verify:  ferriscribe-backup escrow verify --sheet <this-file>\n\
         To restore: ferriscribe-backup restore --escrow-file <this-file> \\\n\
                       --snapshot-dir <snapshot> --dest <empty-dir>\n",
        canary_prefix = &canary[..16],
    );
    let mut f = create_private_file(path)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()
}

/// Write the binary USB escrow artifact: `[FBK1][key 32][canary 32]`.
pub fn write_usb_file(path: &Path, wrapping: &[u8; 32]) -> std::io::Result<()> {
    use std::io::Write;
    let canary = keys::escrow_canary_tag(wrapping);
    let mut out = Vec::with_capacity(USB_MAGIC.len() + 32 + 32);
    out.extend_from_slice(USB_MAGIC);
    out.extend_from_slice(wrapping);
    out.extend_from_slice(&canary);
    let mut f = create_private_file(path)?;
    f.write_all(&out)?;
    f.sync_all()
}

/// Recover the wrapping key from a recovery sheet, verifying its canary.
pub fn read_key_from_sheet(path: &Path) -> BackupResult<[u8; 32]> {
    let text = std::fs::read_to_string(path)?;
    let key_hex = extract_between_markers(&text, SHEET_BEGIN, SHEET_END)?;
    let key_bytes = hex::decode(key_hex).map_err(|e| bad(&format!("sheet key: {e}")))?;
    if key_bytes.len() != 32 {
        return Err(bad("sheet key must be 64 hex characters"));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&key_bytes);

    // Independently verifiable: the sheet's embedded check code must be
    // reproducible from the recovered key.
    let canary_line = extract_after_prefix(&text, "Full check code:")
        .ok_or_else(|| bad("sheet is missing the check code"))?;
    let expected = hex::encode(keys::escrow_canary_tag(&key));
    if !canary_line.eq_ignore_ascii_case(&expected) {
        return Err(bad(
            "check code mismatch — transcription error or tampered sheet",
        ));
    }
    Ok(key)
}

/// Recover the wrapping key from a USB escrow file, verifying its canary.
pub fn read_key_from_usb(path: &Path) -> BackupResult<[u8; 32]> {
    let bytes = std::fs::read(path)?;
    if bytes.len() != USB_MAGIC.len() + 32 + 32 {
        return Err(bad("unexpected escrow file length"));
    }
    if &bytes[..USB_MAGIC.len()] != USB_MAGIC {
        return Err(bad("bad escrow file magic"));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes[USB_MAGIC.len()..USB_MAGIC.len() + 32]);
    let mut canary = [0u8; 32];
    canary.copy_from_slice(&bytes[USB_MAGIC.len() + 32..]);
    if keys::escrow_canary_tag(&key) != canary {
        return Err(bad(
            "check code mismatch — corrupted or tampered escrow file",
        ));
    }
    Ok(key)
}

/// Recover the wrapping key from either artifact form, dispatching on
/// content (USB magic vs. sheet text).
pub fn read_key_from_artifact(path: &Path) -> BackupResult<[u8; 32]> {
    let meta = std::fs::metadata(path)?;
    if meta.len() == (USB_MAGIC.len() + 64) as u64 {
        read_key_from_usb(path)
    } else {
        read_key_from_sheet(path)
    }
}

/// Verify an artifact against the locally-stored wrapping key (if any).
/// Returns a human-facing status line; never includes key material.
pub fn verify_artifact(path: &Path, expected: Option<&[u8; 32]>) -> BackupResult<String> {
    let recovered = read_key_from_artifact(path)?;
    match expected {
        Some(expected) if expected != &recovered => Err(bad(
            "artifact is internally consistent but does NOT match this \
             machine's backup key — was it generated elsewhere?",
        )),
        _ => Ok(format!(
            "escrow artifact verified ({} form)",
            artifact_kind(path)
        )),
    }
}

fn artifact_kind(path: &Path) -> &'static str {
    let is_usb = std::fs::metadata(path)
        .map(|m| m.len() == (USB_MAGIC.len() + 64) as u64)
        .unwrap_or(false);
    if is_usb { "USB" } else { "sheet" }
}

fn extract_between_markers(text: &str, begin: &str, end: &str) -> BackupResult<String> {
    let start = text
        .find(begin)
        .ok_or_else(|| bad("missing begin marker"))?;
    let after = start + begin.len();
    let stop = text[after..]
        .find(end)
        .ok_or_else(|| bad("missing end marker"))?;
    let body = &text[after..after + stop];
    let hex_only: String = body.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex_only.len() != 64 {
        return Err(bad(&format!(
            "expected 64 hex characters between markers, found {}",
            hex_only.len()
        )));
    }
    Ok(hex_only.to_lowercase())
}

fn extract_after_prefix(text: &str, prefix: &str) -> Option<String> {
    let idx = text.find(prefix)?;
    let mut lines = text[idx + prefix.len()..].lines();
    let rest_of_line = lines.next()?.trim();
    let candidate = if rest_of_line.is_empty() {
        lines.next()?.trim()
    } else {
        rest_of_line
    };
    // Ignore the short "first 16" preview line by requiring full length.
    if candidate.len() == 64 {
        Some(candidate.to_lowercase())
    } else {
        None
    }
}

fn bad(msg: &str) -> crate::BackupError {
    crate::BackupError::Escrow(msg.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn sheet_roundtrip_and_verification() {
        let dir = tmp();
        let path = dir.path().join(SHEET_FILENAME);
        let key = [0x11u8; 32];
        write_recovery_sheet(&path, &key).expect("write sheet");
        let recovered = read_key_from_sheet(&path).expect("parse sheet");
        assert_eq!(recovered, key);
    }

    #[test]
    #[cfg(unix)]
    fn escrow_artifacts_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tmp();
        let sheet = dir.path().join(SHEET_FILENAME);
        write_recovery_sheet(&sheet, &[0x11u8; 32]).expect("write sheet");
        let usb = dir.path().join(USB_FILENAME);
        write_usb_file(&usb, &[0x22u8; 32]).expect("write usb");
        for p in [&sheet, &usb] {
            let mode = std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "{} must not be group/other readable", p.display());
        }
    }

    #[test]
    fn usb_roundtrip_and_verification() {
        let dir = tmp();
        let path = dir.path().join(USB_FILENAME);
        let key = [0x22u8; 32];
        write_usb_file(&path, &key).expect("write usb");
        assert_eq!(read_key_from_usb(&path).expect("parse usb"), key);
        // Artifact dispatch picks the USB form.
        assert_eq!(read_key_from_artifact(&path).expect("dispatch"), key);
    }

    #[test]
    fn sheet_with_flipped_hex_char_is_rejected() {
        let dir = tmp();
        let path = dir.path().join(SHEET_FILENAME);
        let key = [0x33u8; 32];
        write_recovery_sheet(&path, &key).expect("write");
        let mut text = std::fs::read_to_string(&path).unwrap();
        // Flip one hex character inside the key block.
        let pos = text.find(SHEET_BEGIN).unwrap() + SHEET_BEGIN.len() + 1;
        let orig = text.as_bytes()[pos];
        let flipped = if orig == b'0' { b'1' } else { b'0' };
        unsafe {
            text.as_bytes_mut()[pos] = flipped;
        }
        std::fs::write(&path, text).unwrap();
        let result = read_key_from_sheet(&path);
        assert!(
            result.is_err(),
            "a single transposed character must fail canary verification"
        );
    }

    #[test]
    fn usb_with_tampered_key_is_rejected() {
        let dir = tmp();
        let path = dir.path().join(USB_FILENAME);
        let key = [0x44u8; 32];
        write_usb_file(&path, &key).expect("write");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[5] ^= 0xFF;
        std::fs::write(&path, bytes).unwrap();
        assert!(read_key_from_usb(&path).is_err());
    }

    #[test]
    fn verify_artifact_detects_foreign_key() {
        let dir = tmp();
        let path = dir.path().join(USB_FILENAME);
        write_usb_file(&path, &[0x55u8; 32]).expect("write");
        // Internally consistent, but generated from a different key than
        // this machine's.
        let result = verify_artifact(&path, Some(&[0x66u8; 32]));
        assert!(result.is_err());
        // And passes when it matches.
        assert!(verify_artifact(&path, Some(&[0x55u8; 32])).is_ok());
    }
}
