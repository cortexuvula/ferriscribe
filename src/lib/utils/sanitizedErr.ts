/// PHI-safe error text for user-facing toasts. Backend error strings can
/// carry row data (a filename IS patient-visible surface in this app), so
/// a raw `${err}` interpolation must never reach the glass. The one shape
/// that is safe to keep is a recording id reference — `recording <uuid>`,
/// the db layer's NotFound wording: ids are opaque, never patient names
/// or content. Everything else collapses to the generic string.

/// `recording 3fa85f64-5717-4562-b3fc-2c963f66afa6` (uuid crate emits
/// lowercase hyphenated; /i tolerates any uppercase round-trips).
const RECORDING_ID_REF =
  /\brecording [0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/i;

export const GENERIC_ERROR_TEXT = 'unexpected error';

/// Extract the message the way it crosses the Tauri invoke boundary:
/// `Error` instances, plain strings, or a serialized `AppError`
/// (`{ kind, message }` — see `impl Serialize for AppError`).
function errMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === 'string') return err;
  if (
    typeof err === 'object' &&
    err !== null &&
    typeof (err as { message?: unknown }).message === 'string'
  ) {
    return (err as { message: string }).message;
  }
  return '';
}

/// Sanitize a caught error for display: keep the `recording <uuid>`
/// reference when the message carries one, otherwise the generic string.
/// Toast copy stays owned by the call site (`Could not restore:
/// ${sanitizedErr(err)}`) — only the interpolated value is sanitized.
export function sanitizedErr(err: unknown): string {
  const match = errMessage(err).match(RECORDING_ID_REF);
  return match ? match[0] : GENERIC_ERROR_TEXT;
}
