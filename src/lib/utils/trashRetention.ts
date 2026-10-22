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
  /// Supplementary tint near eligibility, scaled to the chosen window.
  nearExpiry: boolean;
}

/// Eligibility display for a trashed recording:
/// - readable text with a supplementary tint near expiry;
/// - "Less than 1 day until eligible for permanent deletion" under a day;
/// - "Pending permanent deletion" once past the effective window — NEVER a negative
///   day count and never a promise of an exact deletion time (the purge
///   runs on a daily sweep; the date is eligibility, not an appointment).
export function trashRetentionLabel(
  deletedAtIso: string,
  retentionDays: number,
  now: number = Date.now(),
): RetentionLabel {
  const deletedAt = new Date(deletedAtIso).getTime();
  const eligibleAt = deletedAt + retentionDays * DAY_MS;
  const remaining = eligibleAt - now;
  if (remaining <= 0) {
    return { text: 'Pending permanent deletion', nearExpiry: true };
  }
  const eligibilityDate = new Date(eligibleAt).toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
  });
  if (remaining < DAY_MS) {
    return {
      text: `Less than 1 day until eligible for permanent deletion · ${eligibilityDate}`,
      nearExpiry: true,
    };
  }
  const days = Math.ceil(remaining / DAY_MS);
  const nearExpiryDays = Math.min(7, Math.ceil(retentionDays / 4));
  return {
    text: `${days} ${days === 1 ? 'day' : 'days'} until eligible for permanent deletion · ${eligibilityDate}`,
    nearExpiry: remaining < nearExpiryDays * DAY_MS,
  };
}
