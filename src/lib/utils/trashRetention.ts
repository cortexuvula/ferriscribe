/// Trash retention window in days — mirrors the backend sweeper's literal
/// (`retention_sweep_tick` purges tombstones older than 30 days). Fixed by
/// design (no user setting); do not diverge from the sweeper.
export const TRASH_RETENTION_DAYS = 30;

const DAY_MS = 86_400_000;

/// Date+time for trash rows ("Moved to Trash …", "Recorded …").
export function formatTrashDateTime(iso: string): string {
  return new Date(iso).toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  });
}

export interface RetentionLabel {
  text: string;
  /// Restrained amber treatment near the purge boundary (≤7 days, or
  /// already past it).
  nearExpiry: boolean;
}

/// Days-remaining display for a trashed recording. Rules (brief D4):
/// - neutral text normally, restrained amber near expiry;
/// - "Less than 1 day remaining" under a day;
/// - "Pending permanent deletion" once past 30 days — NEVER a negative
///   day count and never a promise of an exact deletion time (the purge
///   runs on a daily sweep; "after <calendar date>" is the most we can
///   honestly say).
export function trashRetentionLabel(
  deletedAtIso: string,
  now: number = Date.now(),
): RetentionLabel {
  const deletedAt = new Date(deletedAtIso).getTime();
  const purgeAt = deletedAt + TRASH_RETENTION_DAYS * DAY_MS;
  const remaining = purgeAt - now;
  if (remaining <= 0) {
    return { text: 'Pending permanent deletion', nearExpiry: true };
  }
  const purgeAfter = new Date(purgeAt).toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
  });
  if (remaining < DAY_MS) {
    return {
      text: `Less than 1 day remaining · Permanent deletion after ${purgeAfter}`,
      nearExpiry: true,
    };
  }
  const days = Math.ceil(remaining / DAY_MS);
  return {
    text: `${days} ${days === 1 ? 'day' : 'days'} remaining · Permanent deletion after ${purgeAfter}`,
    nearExpiry: remaining < 7 * DAY_MS,
  };
}
