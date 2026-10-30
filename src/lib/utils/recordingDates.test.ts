import { describe, it, expect, vi } from 'vitest';
import {
  localDateKey,
  parseLocalDateKey,
  nextDayKey,
  dayStartUtcForKey,
  dayIntervalUtcForKey,
  groupCreationTimesByDay,
  fullLocalDateFromKey,
  isValidDayKey,
} from './recordingDates';

const HOUR = 3_600_000;

/// The 2026 DST transitions every fixed-zone assertion below leans on
/// (verified against ICU, not the host): America/New_York springs forward
/// Sun 2026-03-08 02:00→03:00 (EST −5 → EDT −4) and falls back Sun
/// 2026-11-01 02:00→01:00. Lord Howe shifts by 30 minutes (+10:30 → +11:00
/// on Sun 2026-10-04; back on Sun 2026-04-05).
const NY = 'America/New_York';

describe('localDateKey — local calendar key from an instant', () => {
  it('derives the key from the named zone, not the host zone (offset handling)', () => {
    // 00:30 UTC on Oct 5 is still Oct 4 evening in New York (20:30 EDT).
    // The same instant must key differently per zone — this fails if the
    // implementation ever falls back to the host zone or a UTC substring.
    expect(localDateKey(new Date('2026-10-05T00:30:00Z'), NY)).toBe('2026-10-04');
    expect(localDateKey(new Date('2026-10-05T00:30:00Z'), 'UTC')).toBe('2026-10-05');
    // Positive offsets roll forward: 15:30 UTC is 00:30 Oct 6 in Tokyo (+9).
    expect(localDateKey(new Date('2026-10-05T15:30:00Z'), 'Asia/Tokyo')).toBe('2026-10-06');
    // +14 (Kiritimati): 10:30 UTC Oct 5 is already Oct 6.
    expect(localDateKey(new Date('2026-10-05T10:30:00Z'), 'Pacific/Kiritimati')).toBe('2026-10-06');
  });

  it('accepts RFC3339 strings with explicit offsets, keyed by the actual instant', () => {
    // 23:30 at −05:00 is 04:30 UTC Oct 6 — Tokyo files it on Oct 6, New
    // York (which is −4 in October, so −05:00 is a Chicago-style stamp)
    // sees 03:30 Oct 6 local. What matters: the offset RESOLVES before
    // keying; no substring of the raw string is trusted.
    expect(localDateKey('2026-10-05T23:30:00-05:00', 'Asia/Tokyo')).toBe('2026-10-06');
    expect(localDateKey('2026-10-05T23:30:00-05:00', 'UTC')).toBe('2026-10-06');
  });

  it('rejects unparseable timestamps loudly', () => {
    expect(() => localDateKey('not-a-timestamp', NY)).toThrow();
    expect(() => localDateKey(new Date(NaN), NY)).toThrow();
  });

  it('classifies an instant exactly at local midnight into its own day', () => {
    // 04:00 UTC is New York midnight on Oct 5 (EDT −4) — start-inclusive.
    expect(localDateKey(new Date('2026-10-05T04:00:00Z'), NY)).toBe('2026-10-05');
    // One millisecond earlier is the previous day.
    expect(localDateKey(new Date('2026-10-05T03:59:59.999Z'), NY)).toBe('2026-10-04');
  });
});

describe('parseLocalDateKey — local-midnight parser without the UTC-string trap', () => {
  it('parses to LOCAL midnight of the runtime zone', () => {
    const d = parseLocalDateKey('2026-10-05');
    expect(d.getFullYear()).toBe(2026);
    expect(d.getMonth()).toBe(9);
    expect(d.getDate()).toBe(5);
    expect(d.getHours()).toBe(0);
    expect(d.getMinutes()).toBe(0);
    // The exact anti-trap property: the parser must equal the explicit
    // local-components constructor, never `new Date('2026-10-05')` which
    // the ES spec parses as UTC MIDNIGHT (a different instant in every
    // nonzero-offset runtime zone).
    expect(d.getTime()).toBe(new Date(2026, 9, 5).getTime());
  });

  it('rejects malformed keys', () => {
    expect(() => parseLocalDateKey('2026-10')).toThrow();
    expect(() => parseLocalDateKey('2026-10-05T00:00:00Z')).toThrow();
    expect(() => parseLocalDateKey('garbage')).toThrow();
  });
});

describe('nextDayKey — calendar rollover', () => {
  it('rolls month and year boundaries', () => {
    expect(nextDayKey('2026-10-05')).toBe('2026-10-06');
    expect(nextDayKey('2026-10-31')).toBe('2026-11-01');
    expect(nextDayKey('2026-12-31')).toBe('2027-01-01');
    expect(nextDayKey('2027-01-31')).toBe('2027-02-01');
    // Leap day: 2028 is a leap year.
    expect(nextDayKey('2028-02-28')).toBe('2028-02-29');
    expect(nextDayKey('2028-02-29')).toBe('2028-03-01');
  });
});

describe('dayStartUtcForKey / dayIntervalUtcForKey — DST-safe day bounds', () => {
  it('New York spring-forward day 2026-03-08 is exactly 23 hours', () => {
    const { startIso, endIso } = dayIntervalUtcForKey('2026-03-08', NY);
    // Local midnight before the jump is EST (−5) → 05:00Z; the next
    // midnight is EDT (−4) → 04:00Z. 04:00Z−05:00Z = 23h.
    expect(startIso).toBe('2026-03-08T05:00:00.000Z');
    expect(endIso).toBe('2026-03-09T04:00:00.000Z');
    expect(new Date(endIso).getTime() - new Date(startIso).getTime()).toBe(23 * HOUR);
  });

  it('New York fall-back day 2026-11-01 is exactly 25 hours', () => {
    const { startIso, endIso } = dayIntervalUtcForKey('2026-11-01', NY);
    expect(startIso).toBe('2026-11-01T04:00:00.000Z'); // EDT until 02:00
    expect(endIso).toBe('2026-11-02T05:00:00.000Z'); // EST after the jump
    expect(new Date(endIso).getTime() - new Date(startIso).getTime()).toBe(25 * HOUR);
  });

  it('ordinary New York days are 24 hours, neighbours of the transitions included', () => {
    expect(new Date(dayIntervalUtcForKey('2026-03-07', NY).endIso).getTime()
      - new Date(dayIntervalUtcForKey('2026-03-07', NY).startIso).getTime()).toBe(24 * HOUR);
    expect(new Date(dayIntervalUtcForKey('2026-11-02', NY).endIso).getTime()
      - new Date(dayIntervalUtcForKey('2026-11-02', NY).startIso).getTime()).toBe(24 * HOUR);
  });

  it('Lord Howe 30-minute DST yields 23.5h/24.5h days (minute-level precision)', () => {
    const LHI = 'Australia/Lord_Howe';
    const spring = dayIntervalUtcForKey('2026-10-04', LHI);
    const fall = dayIntervalUtcForKey('2026-04-05', LHI);
    expect(new Date(spring.endIso).getTime() - new Date(spring.startIso).getTime())
      .toBe(23.5 * HOUR);
    expect(new Date(fall.endIso).getTime() - new Date(fall.startIso).getTime())
      .toBe(24.5 * HOUR);
  });

  it('extreme offsets: Kiritimati (+14) and Tokyo (+9, no DST) day starts', () => {
    expect(dayStartUtcForKey('2026-10-05', 'Pacific/Kiritimati').toISOString())
      .toBe('2026-10-04T10:00:00.000Z');
    expect(dayStartUtcForKey('2026-10-05', 'Asia/Tokyo').toISOString())
      .toBe('2026-10-04T15:00:00.000Z');
  });

  it('year rollover: Dec 31 ends exactly where Jan 1 begins', () => {
    const dec31 = dayIntervalUtcForKey('2026-12-31', NY);
    const jan1 = dayIntervalUtcForKey('2027-01-01', NY);
    expect(dec31.endIso).toBe(jan1.startIso);
    expect(new Date(dec31.endIso).toISOString()).toBe('2027-01-01T05:00:00.000Z');
  });

  it('the half-open [start, end) interval classifies boundary instants correctly', () => {
    const { startIso, endIso } = dayIntervalUtcForKey('2026-03-08', NY);
    const start = new Date(startIso).getTime();
    const end = new Date(endIso).getTime();
    // Start inclusive, end exclusive — an instant 1ms before the end is
    // still Mar 8; at the end it is already Mar 9.
    expect(localDateKey(new Date(start), NY)).toBe('2026-03-08');
    expect(localDateKey(new Date(end - 1), NY)).toBe('2026-03-08');
    expect(localDateKey(new Date(end), NY)).toBe('2026-03-09');
  });
});

describe('groupCreationTimesByDay — availability grouping', () => {
  it('groups whole timestamps into local-day counts, duplicates retained', () => {
    const counts = groupCreationTimesByDay(
      [
        '2026-10-05T04:00:00Z', // NY midnight Oct 5
        '2026-10-05T23:00:00Z', // 19:00 NY, still Oct 5
        '2026-10-06T03:59:00Z', // 23:59 NY, STILL Oct 5 — one minute from the edge
        '2026-10-06T04:00:00Z', // NY midnight Oct 6
        '2026-10-06T04:00:00Z', // duplicate instant — count 2 on the day
      ],
      NY,
    );
    expect(counts.get('2026-10-05')).toBe(3);
    expect(counts.get('2026-10-06')).toBe(2);
    expect(counts.size).toBe(2);
  });

  it('accepts Date objects alongside strings', () => {
    const counts = groupCreationTimesByDay(
      [new Date('2026-10-05T12:00:00Z'), '2026-10-05T18:00:00Z'],
      'UTC',
    );
    expect(counts.get('2026-10-05')).toBe(2);
  });

  it('throws on an unreadable timestamp instead of silently dropping it', () => {
    // Empty availability must mean "no recordings", never "some rows were
    // unreadable" — mirroring the backend's propagate-don't-drop contract.
    expect(() => groupCreationTimesByDay(['2026-10-05T04:00:00Z', 'corrupt'], NY)).toThrow();
  });

  it('an empty input groups to an empty map', () => {
    expect(groupCreationTimesByDay([], NY).size).toBe(0);
  });
});

describe('isValidDayKey / fullLocalDateFromKey', () => {
  it('validates the strict YYYY-MM-DD shape', () => {
    expect(isValidDayKey('2026-10-05')).toBe(true);
    expect(isValidDayKey('2026-1-5')).toBe(false);
    expect(isValidDayKey('2026-10-5')).toBe(false);
    expect(isValidDayKey('')).toBe(false);
    expect(isValidDayKey('2026-10-05x')).toBe(false);
  });

  it('formats a key as a full local date for notices (no UTC off-by-one day)', () => {
    const text = fullLocalDateFromKey('2026-10-05');
    const d = parseLocalDateKey('2026-10-05');
    // Same calendar fields as the key — a UTC-string format of the key
    // would shift the day in nonzero-offset zones.
    expect(text).toBe(
      d.toLocaleDateString(undefined, { year: 'numeric', month: 'long', day: 'numeric' }),
    );
    expect(text).toContain('2026');
  });
});

describe('runtime-zone default agrees across the helper surface', () => {
  it('key → interval → key round-trips for the runtime zone', () => {
    // Host-zone-independent property: whatever the runtime zone is, the
    // interval built for a key covers exactly the instants that key back
    // to that day, and the day-start is a wall-clock midnight.
    for (const key of ['2026-03-08', '2026-10-05', '2026-12-31']) {
      const { startIso, endIso } = dayIntervalUtcForKey(key);
      const start = new Date(startIso);
      const end = new Date(endIso);
      expect(localDateKey(start)).toBe(key);
      expect(localDateKey(new Date(end.getTime() - 1))).toBe(key);
      expect(localDateKey(end)).toBe(nextDayKey(key));
      // A calendar day is 23–25h even under DST (Lord-Howe-style 30-min
      // zones sit inside this range too: 23.5h/24.5h).
      const hours = (end.getTime() - start.getTime()) / HOUR;
      expect(hours).toBeGreaterThanOrEqual(23);
      expect(hours).toBeLessThanOrEqual(25);
    }
  });
});

// ── S-4: canonical day-key contract (locale-independent construction) ───
//
// Codie's suggestion: the day key must be CONSTRUCTED from typed
// formatToParts output, never assembled from the locale-formatted
// `format()` string — part order and separators are locale/ICU artifacts.
// A formatter whose output cannot form a canonical key must fail LOUDLY,
// never fall back to a silently-corrupted key (which would misfile every
// recording into wrong calendar days).

describe('localDateKey — canonical key construction (S-4)', () => {
  it('derives the key from typed parts even when format() output is not YYYY-MM-DD ordered', async () => {
    // A hostile formatter whose format() renders day-first garbage while
    // its formatToParts stay honest — the shape a future ICU/locale
    // surprise would take. Re-import the module fresh so its formatter
    // cache is built against the stub.
    const RealDTF = Intl.DateTimeFormat;
    class ReorderedDTF {
      private inner: Intl.DateTimeFormat;
      constructor(...args: ConstructorParameters<typeof Intl.DateTimeFormat>) {
        this.inner = new RealDTF(...args);
      }
      format(): string {
        return '05/10/2026'; // deliberately NOT a canonical key
      }
      formatToParts(date?: Date | number): Intl.DateTimeFormatPart[] {
        return this.inner.formatToParts(date);
      }
      resolvedOptions(): Intl.ResolvedDateTimeFormatOptions {
        return this.inner.resolvedOptions();
      }
    }
    vi.resetModules();
    try {
      Intl.DateTimeFormat = ReorderedDTF as unknown as typeof Intl.DateTimeFormat;
      const { localDateKey: freshKey } = await import('./recordingDates');
      expect(freshKey(new Date('2026-10-05T12:00:00Z'), 'UTC')).toBe('2026-10-05');
      expect(freshKey(new Date('2026-10-06T00:30:00Z'), NY)).toBe('2026-10-05');
    } finally {
      Intl.DateTimeFormat = RealDTF;
    }
  });

  it('throws loudly when the formatter cannot produce canonical parts — no fake success', async () => {
    const RealDTF = Intl.DateTimeFormat;
    class BrokenPartsDTF {
      format(): string {
        return 'whenever';
      }
      formatToParts(): Intl.DateTimeFormatPart[] {
        // No 'year' part: no canonical key can be constructed.
        return [
          { type: 'month', value: '10' },
          { type: 'day', value: '5' },
          { type: 'literal', value: '/' },
        ];
      }
      resolvedOptions(): Intl.ResolvedDateTimeFormatOptions {
        return new RealDTF().resolvedOptions();
      }
    }
    vi.resetModules();
    try {
      Intl.DateTimeFormat = BrokenPartsDTF as unknown as typeof Intl.DateTimeFormat;
      const { localDateKey: freshKey } = await import('./recordingDates');
      expect(() => freshKey(new Date('2026-10-05T12:00:00Z'), 'UTC')).toThrow();
    } finally {
      Intl.DateTimeFormat = RealDTF;
    }
  });

  it('real formatter output is canonical across a boundary sweep (contract pin)', () => {
    // Green before AND after the fix — pins the contract itself: every
    // key is strict zero-padded YYYY-MM-DD (lexicographic == chronological).
    for (const ts of [
      '2026-12-31T23:59:59.999Z',
      '2027-01-01T00:00:00Z',
      '2026-02-28T12:00:00Z',
      '2026-03-01T00:00:00Z',
    ]) {
      for (const zone of ['UTC', NY, 'Pacific/Kiritimati']) {
        const key = localDateKey(new Date(ts), zone);
        expect(isValidDayKey(key)).toBe(true);
      }
    }
  });
});
