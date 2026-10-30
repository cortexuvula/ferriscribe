/// Local-calendar date helpers for the recordings date filter
/// (recording-date-calendar, batch 2).
///
/// The backend deliberately projects ONLY timezone-free UTC instants
/// (`list_recording_creation_times`); grouping into local calendar days,
/// day keys, and day bounds happens here in the browser's own timezone —
/// never via UTC substrings (a `created_at.slice(0, 10)` misfiles every
/// recording made between local evening and UTC midnight) and never via a
/// single fixed offset (DST moves the offset twice a year).
///
/// Every helper takes an optional IANA zone name defaulting to the runtime
/// zone. Tests exercise FIXED zones (`America/New_York`,
/// `Australia/Lord_Howe`, …) through the same code paths the store runs,
/// so DST behavior is proven rather than assumed from the host.

const HOUR_MS = 3_600_000;
/// Real zone offsets span UTC−12…UTC+14, so a day's start instant lies
/// within ±15h of its wall date read as UTC. This bounds the binary
/// search in [`dayStartUtcForKey`] and guarantees its window.
const MAX_ZONE_OFFSET_HOURS = 15;

/// The browser's own IANA zone (e.g. 'America/Vancouver').
export function runtimeTimeZone(): string {
  return Intl.DateTimeFormat().resolvedOptions().timeZone;
}

const dayKeyFormatters = new Map<string, Intl.DateTimeFormat>();

/// Cached per-zone day-key formatter. Only its typed `formatToParts`
/// output is consumed (see canonicalDayKey) — never its `format()` string,
/// whose part ordering and separators are locale/ICU artifacts a future
/// ICU update could reshape.
function dayKeyFormatter(zone: string): Intl.DateTimeFormat {
  let f = dayKeyFormatters.get(zone);
  if (!f) {
    f = new Intl.DateTimeFormat('en-CA', {
      timeZone: zone,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
    });
    dayKeyFormatters.set(zone, f);
  }
  return f;
}

/// Canonical `YYYY-MM-DD` key built from the formatter's TYPED parts, not
/// its formatted string: part order and separators never enter the key.
/// Zero-padding keeps the keys lexicographically ordered, which the
/// dayStartUtcForKey binary search relies on. Formatter output that
/// cannot form a canonical key THROWS — loudly — because silently using a
/// corrupted key would misfile recordings into wrong calendar days (a
/// fake successful fallback is never acceptable here).
function canonicalDayKey(formatter: Intl.DateTimeFormat, instant: Date): string {
  const parts = formatter.formatToParts(instant);
  let year = '';
  let month = '';
  let day = '';
  for (const p of parts) {
    if (p.type === 'year') year = p.value;
    else if (p.type === 'month') month = p.value;
    else if (p.type === 'day') day = p.value;
  }
  const key = `${year}-${month.padStart(2, '0')}-${day.padStart(2, '0')}`;
  if (!isValidDayKey(key)) {
    throw new Error(`day-key formatter produced a non-canonical key: ${key}`);
  }
  return key;
}

/// Strict `YYYY-MM-DD` day-key shape (zero-padded, nothing extra).
export function isValidDayKey(key: string): boolean {
  return /^\d{4}-\d{2}-\d{2}$/.test(key);
}

function dayKeyParts(key: string): [number, number, number] {
  if (!isValidDayKey(key)) {
    throw new Error(`invalid day key: expected strict YYYY-MM-DD`);
  }
  const [y, m, d] = key.split('-').map(Number);
  return [y, m, d];
}

function toInstant(instant: Date | string): Date {
  const d = instant instanceof Date ? instant : new Date(instant);
  if (Number.isNaN(d.getTime())) {
    throw new Error('invalid timestamp');
  }
  return d;
}

/// The `YYYY-MM-DD` calendar key of `instant`'s day in `zone` (default:
/// the runtime zone). Accepts RFC3339 strings with any offset — the
/// offset resolves to the actual instant BEFORE keying. The key is built
/// from typed formatToParts (see canonicalDayKey), so it is canonical by
/// construction regardless of locale/ICU formatting drift.
export function localDateKey(instant: Date | string, zone: string = runtimeTimeZone()): string {
  return canonicalDayKey(dayKeyFormatter(zone), toInstant(instant));
}

/// Parse a strict `YYYY-MM-DD` key to LOCAL MIDNIGHT of the runtime zone.
///
/// Deliberately NOT `new Date(key)`: per the ES spec a bare `YYYY-MM-DD`
/// string parses as UTC midnight — in every nonzero-offset zone that is
/// the previous local evening, and building day bounds from it would
/// misfile boundary recordings ("the UTC-Date-string trap").
export function parseLocalDateKey(key: string): Date {
  const [y, m, d] = dayKeyParts(key);
  return new Date(y, m - 1, d);
}

/// The calendar day after `key` (pure arithmetic — rolls month, year, and
/// leap-day boundaries; zone-free by construction).
export function nextDayKey(key: string): string {
  const [y, m, d] = dayKeyParts(key);
  // Date.UTC normalizes overflow (Jan 32 → Feb 1); formatting the rolled
  // instant in the UTC zone sidesteps the runtime zone entirely.
  return localDateKey(new Date(Date.UTC(y, m - 1, d + 1)), 'UTC');
}

/// The UTC instant at which calendar day `key` begins in `zone` (local
/// midnight — DST-adjusted, so a spring-forward day starts after a
/// skipped hour and a fall-back day after a repeated one).
///
/// Found by binary search over the ±15h window the day's start must lie
/// in: day keys are monotone in the instant and lexicographically
/// ordered, so the first millisecond whose key equals `key` IS the
/// boundary. This stays exact for every zone shape — midnight
/// transitions, 30-minute DST (Lord Howe), even whole skipped days —
/// where offset-arithmetic approaches need special cases.
export function dayStartUtcForKey(key: string, zone: string = runtimeTimeZone()): Date {
  const [y, m, d] = dayKeyParts(key);
  const naive = Date.UTC(y, m - 1, d);
  let lo = naive - MAX_ZONE_OFFSET_HOURS * HOUR_MS;
  let hi = naive + MAX_ZONE_OFFSET_HOURS * HOUR_MS;
  // Invariant: localDateKey(lo) < key < localDateKey-ordered ≤ key at hi —
  // the window covers every real offset (±14h max), with an hour to spare.
  while (hi - lo > 1) {
    const mid = lo + Math.floor((hi - lo) / 2);
    if (localDateKey(new Date(mid), zone) < key) {
      lo = mid;
    } else {
      hi = mid;
    }
  }
  return new Date(hi);
}

/// The half-open `[startIso, endIso)` UTC interval covering calendar day
/// `key` in `zone` (default: the runtime zone) — the same semantics as
/// the store's `localDayIntervalUtc` (start inclusive, end exclusive,
/// both bounds local midnights), so the date filter and the Trash
/// restore-by-date window classify boundary instants identically.
export function dayIntervalUtcForKey(
  key: string,
  zone: string = runtimeTimeZone(),
): { startIso: string; endIso: string } {
  const start = dayStartUtcForKey(key, zone);
  const end = dayStartUtcForKey(nextDayKey(key), zone);
  return { startIso: start.toISOString(), endIso: end.toISOString() };
}

/// Group whole-timestamp availability (one creation instant per active
/// recording, duplicates retained) into per-local-day counts in `zone`.
///
/// An unreadable timestamp THROWS rather than being dropped: an empty
/// calendar must mean "no recordings", never "some rows were unreadable"
/// — mirroring the backend's propagate-don't-drop contract.
export function groupCreationTimesByDay(
  times: readonly (string | Date)[],
  zone: string = runtimeTimeZone(),
): Map<string, number> {
  const counts = new Map<string, number>();
  for (const t of times) {
    const key = localDateKey(t, zone);
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  return counts;
}

/// A key formatted as a full local date (e.g. "October 5, 2026") for
/// user-facing notices — parsed through [`parseLocalDateKey`], so the
/// notice names the same calendar day the filter used (never a UTC
/// off-by-one day).
export function fullLocalDateFromKey(key: string): string {
  return parseLocalDateKey(key).toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'long',
    day: 'numeric',
  });
}
