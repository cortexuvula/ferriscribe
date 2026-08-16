/** Format seconds as MM:SS for duration display. */
export function formatDuration(seconds: number | null): string {
  if (seconds === null) return '--:--';
  const m = Math.floor(seconds / 60).toString().padStart(2, '0');
  const s = Math.floor(seconds % 60).toString().padStart(2, '0');
  return `${m}:${s}`;
}

/** Format an ISO timestamp to a locale time string (HH:MM). */
export function formatTimestamp(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
}

/** Format an ISO date to a short locale date string (e.g. "Apr 14, 2026"). */
export function formatDate(iso: string): string {
  const d = new Date(iso);
  return d.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    year: 'numeric',
  });
}

/** Format a tokens-per-second value as a compact label (e.g. for a meta row). */
export function formatTokensPerSecond(v: number | null | undefined): string {
  if (v === null || v === undefined || !Number.isFinite(v) || v < 0) return '';
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} tok/s`;
}
