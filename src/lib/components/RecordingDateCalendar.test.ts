// @vitest-environment jsdom
/**
 * RecordingDateCalendar — populated-only date picker popover
 * (recording-date-calendar, batch 3).
 *
 * Facts pinned:
 *   - Trigger carries aria-expanded/aria-controls; the popover is a
 *     labelled nonmodal dialog that opens/closes.
 *   - ONLY populated days render as numbered buttons (full-date + count
 *     accessible names); blanks are inert and aria-hidden. No role=grid.
 *   - Year/month selects offer ONLY populated years, and only the populated
 *     months of the chosen year. Prev/next jump between POPULATED months
 *     and disable at the endpoints.
 *   - Initial view = the selected day's month (when still populated), else
 *     the newest populated month. No automatic selection.
 *   - Selecting a day calls onSelectDate, closes the popover, and restores
 *     trigger focus. Escape closes + restores focus WITHOUT changing the
 *     selection; an outside click closes without selection changes.
 *   - Open focuses the selected day, else the first available day.
 *     A refresh that removes the focused day recovers focus.
 *   - Loading/error/empty are distinct; a failed availability fetch is
 *     retryable and NEVER renders as "No recordings yet".
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/svelte';
import RecordingDateCalendar from './RecordingDateCalendar.svelte';
import { fullLocalDateFromKey } from '../utils/recordingDates';

const onSelectDate = vi.fn();
const onRefreshAvailability = vi.fn();

function counts(entries: [string, number][]): Map<string, number> {
  return new Map(entries);
}

/// Populated days across three years: 2025-11-20, 2026-03-02, and
/// 2026-10-05 (2 recordings) + 2026-10-06 (1).
const FIXTURE = counts([
  ['2025-11-20', 3],
  ['2026-03-02', 1],
  ['2026-10-05', 2],
  ['2026-10-06', 1],
]);

function baseProps(overrides: Record<string, unknown> = {}) {
  return {
    availabilityCounts: FIXTURE,
    availabilityLoading: false,
    availabilityError: null,
    availabilityLoaded: true,
    selectedDate: null,
    onSelectDate,
    onRefreshAvailability,
    ...overrides,
  };
}

function dayButtonName(key: string, n: number): string {
  return `${fullLocalDateFromKey(key)}, ${n} recording${n === 1 ? '' : 's'}`;
}

function trigger(): HTMLButtonElement {
  return screen.getByRole('button', { name: 'Choose recording date' }) as HTMLButtonElement;
}

async function openCalendar(props: Record<string, unknown> = {}) {
  const rendered = render(RecordingDateCalendar, baseProps(props));
  await fireEvent.click(trigger());
  expect(screen.getByRole('dialog')).toBeTruthy();
  return rendered;
}

beforeEach(() => {
  vi.clearAllMocks();
  cleanup();
});
afterEach(cleanup);

describe('RecordingDateCalendar — trigger + popover structure', () => {
  it('trigger carries aria-expanded/aria-controls pointing at the labelled popover', async () => {
    render(RecordingDateCalendar, baseProps());
    const btn = trigger();
    expect(btn.getAttribute('aria-expanded')).toBe('false');
    const controlledId = btn.getAttribute('aria-controls');
    expect(controlledId).toBeTruthy();

    await fireEvent.click(btn);
    expect(btn.getAttribute('aria-expanded')).toBe('true');
    const popover = screen.getByRole('dialog');
    expect(popover.id).toBe(controlledId);
    expect(popover.getAttribute('aria-modal')).toBe('false');
    expect(popover.getAttribute('aria-labelledby')).toBeTruthy();
    // Labelled by the visible "Choose recording date" heading.
    const labelId = popover.getAttribute('aria-labelledby')!;
    expect(document.getElementById(labelId)?.textContent).toContain('Choose recording date');

    await fireEvent.click(btn); // toggle closed
    expect(btn.getAttribute('aria-expanded')).toBe('false');
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('opening refreshes availability (timezone changes rederive); closing does not', async () => {
    await openCalendar();
    expect(onRefreshAvailability).toHaveBeenCalledTimes(1);

    await fireEvent.click(trigger()); // close via toggle
    expect(onRefreshAvailability).toHaveBeenCalledTimes(1);

    await fireEvent.click(trigger()); // reopen
    expect(onRefreshAvailability).toHaveBeenCalledTimes(2);
  });

  it('uses ordinary button semantics — no grid/dialog-per-cell roles', async () => {
    await openCalendar();
    expect(screen.queryByRole('grid')).toBeNull();
    expect(screen.queryByRole('gridcell')).toBeNull();
    // Day buttons exist with full-date accessible names.
    expect(
      screen.getByRole('button', { name: dayButtonName('2026-10-05', 2) }),
    ).toBeTruthy();
  });
});

describe('RecordingDateCalendar — populated-only days, months, years', () => {
  it('renders ONLY populated days as buttons; blanks are inert and aria-hidden', async () => {
    await openCalendar();
    // Initial view = newest populated month (2026-10).
    expect(screen.getByRole('button', { name: dayButtonName('2026-10-05', 2) })).toBeTruthy();
    expect(screen.getByRole('button', { name: dayButtonName('2026-10-06', 1) })).toBeTruthy();
    // Empty positions in the month are NOT buttons.
    expect(screen.queryByRole('button', { name: dayButtonName('2026-10-04', 1) })).toBeNull();
    expect(screen.queryByRole('button', { name: /October 7/ })).toBeNull();
    expect(screen.queryByRole('button', { name: /October 3/ })).toBeNull();
  });

  it('year select offers ONLY populated years; month select ONLY populated months of that year', async () => {
    await openCalendar();
    const yearSelect = screen.getByLabelText('Year') as HTMLSelectElement;
    expect([...yearSelect.options].map((o) => o.value)).toEqual(['2025', '2026']);

    // 2026 is populated in March (month index 2) and October (index 9) only.
    const monthSelect = screen.getByLabelText('Month') as HTMLSelectElement;
    expect([...monthSelect.options].map((o) => o.value)).toEqual(['2', '9']);

    // Jump to 2025 — November is its only populated month, and the day grid
    // moves with it.
    await fireEvent.change(yearSelect, { target: { value: '2025' } });
    expect(
      screen.getByRole('button', { name: dayButtonName('2025-11-20', 3) }),
    ).toBeTruthy();
    expect(screen.queryByRole('button', { name: /October/ })).toBeNull();
    const monthSelect2025 = screen.getByLabelText('Month') as HTMLSelectElement;
    expect([...monthSelect2025.options].map((o) => o.value)).toEqual(['10']);

    // Back to 2026, then the month select jumps within the year: choosing
    // March shows March's (only) day.
    await fireEvent.change(screen.getByLabelText('Year'), { target: { value: '2026' } });
    await fireEvent.change(screen.getByLabelText('Month'), { target: { value: '2' } });
    expect(screen.getByRole('button', { name: dayButtonName('2026-03-02', 1) })).toBeTruthy();
  });

  it('prev/next jump between POPULATED months and disable at the endpoints', async () => {
    await openCalendar(); // newest = 2026-10
    const prev = screen.getByRole('button', { name: 'Previous month with recordings' }) as HTMLButtonElement;
    const next = screen.getByRole('button', { name: 'Next month with recordings' }) as HTMLButtonElement;
    expect(next.disabled).toBe(true); // at the newest populated month

    await fireEvent.click(prev); // 2026-03 (skips unpopulated months)
    expect(screen.getByRole('button', { name: dayButtonName('2026-03-02', 1) })).toBeTruthy();
    await fireEvent.click(prev); // 2025-11
    expect(screen.getByRole('button', { name: dayButtonName('2025-11-20', 3) })).toBeTruthy();
    expect((screen.getByRole('button', { name: 'Previous month with recordings' }) as HTMLButtonElement).disabled).toBe(true);

    await fireEvent.click(screen.getByRole('button', { name: 'Next month with recordings' }));
    expect(screen.getByRole('button', { name: dayButtonName('2026-03-02', 1) })).toBeTruthy();
  });
});

describe('RecordingDateCalendar — initial view + selection', () => {
  it('opens on the selected day\'s month (when populated) with NO automatic selection', async () => {
    await openCalendar({ selectedDate: '2026-03-02' });
    expect(screen.getByRole('button', { name: dayButtonName('2026-03-02', 1) })).toBeTruthy();
    expect(
      (screen.getByRole('button', { name: dayButtonName('2026-03-02', 1) }) as HTMLButtonElement).getAttribute('aria-pressed'),
    ).toBe('true');
    expect(onSelectDate).not.toHaveBeenCalled();
  });

  it('falls back to the newest populated month when the selected day is not populated', async () => {
    await openCalendar({ selectedDate: '2026-01-15' });
    expect(screen.getByRole('button', { name: dayButtonName('2026-10-05', 2) })).toBeTruthy();
  });

  it('selecting a day calls onSelectDate, closes the popover, and restores trigger focus', async () => {
    await openCalendar();
    await fireEvent.click(screen.getByRole('button', { name: dayButtonName('2026-10-05', 2) }));
    expect(onSelectDate).toHaveBeenCalledWith('2026-10-05');
    expect(onSelectDate).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(trigger().getAttribute('aria-expanded')).toBe('false');
    expect(document.activeElement).toBe(trigger());
  });

  it('the selected day carries a non-color cue (✓) alongside aria-pressed', async () => {
    await openCalendar({ selectedDate: '2026-10-05' });
    const btn = screen.getByRole('button', { name: dayButtonName('2026-10-05', 2) });
    expect(btn.getAttribute('aria-pressed')).toBe('true');
    expect(btn.textContent).toContain('✓');
    // A different populated day is not marked selected.
    const other = screen.getByRole('button', { name: dayButtonName('2026-10-06', 1) });
    expect(other.getAttribute('aria-pressed')).toBe('false');
    expect(other.textContent).not.toContain('✓');
  });
});

describe('RecordingDateCalendar — keyboard + focus', () => {
  it('open focuses the selected day', async () => {
    await openCalendar({ selectedDate: '2026-10-06' });
    expect(document.activeElement).toBe(
      screen.getByRole('button', { name: dayButtonName('2026-10-06', 1) }),
    );
  });

  it('open with no selection focuses the first available day', async () => {
    await openCalendar();
    expect(document.activeElement).toBe(
      screen.getByRole('button', { name: dayButtonName('2026-10-05', 2) }),
    );
  });

  it('Escape closes, restores trigger focus, and changes nothing', async () => {
    await openCalendar({ selectedDate: '2026-10-05' });
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(document.activeElement).toBe(trigger());
    expect(onSelectDate).not.toHaveBeenCalled();
  });

  it('an outside click closes without selection changes or focus yanking', async () => {
    await openCalendar();
    await fireEvent.click(document.body);
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(onSelectDate).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(document.body);
  });

  it('a refresh that removes the focused day recovers focus onto an available day', async () => {
    const { rerender } = await openCalendar();
    expect(document.activeElement).toBe(
      screen.getByRole('button', { name: dayButtonName('2026-10-05', 2) }),
    );

    // Availability refresh lands without 2026-10-05 (e.g. that day's last
    // recording was deleted on another view). The focused button vanishes.
    await rerender(baseProps({ availabilityCounts: counts([['2026-10-06', 1]]) }));

    await waitFor(() => {
      expect(document.activeElement).toBe(
        screen.getByRole('button', { name: dayButtonName('2026-10-06', 1) }),
      );
    });
    // Still open — focus recovery, not a close.
    expect(screen.getByRole('dialog')).toBeTruthy();
  });

  it('keeps the view month populated after counts change (jumps when its days empty)', async () => {
    const { rerender } = await openCalendar(); // view = 2026-10
    await rerender(baseProps({ availabilityCounts: counts([['2026-03-02', 1]]) }));
    // October emptied — the view lands on the nearest populated month with
    // focus recovered, never an empty grid.
    await waitFor(() =>
      expect(screen.getByRole('button', { name: dayButtonName('2026-03-02', 1) })).toBeTruthy(),
    );
    expect(screen.queryByRole('button', { name: /October/ })).toBeNull();
  });
});

describe('RecordingDateCalendar — loading / error / empty states', () => {
  it('loading with no data shows the loading status, never an empty success', async () => {
    await openCalendar({ availabilityCounts: counts([]), availabilityLoading: true, availabilityLoaded: false });
    expect(screen.getByText('Loading recording dates…')).toBeTruthy();
    expect(screen.queryByText('No recordings yet.')).toBeNull();
    expect(screen.queryByLabelText('Year')).toBeNull();
  });

  it('a failed fetch with no data is retryable and NEVER reads as "No recordings yet"', async () => {
    await openCalendar({ availabilityCounts: counts([]), availabilityError: 'unexpected error' });
    expect(screen.getByText("Couldn't load recording dates.")).toBeTruthy();
    expect(screen.queryByText('No recordings yet.')).toBeNull();

    await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    expect(onRefreshAvailability).toHaveBeenCalledTimes(2); // once on open, once on retry
  });

  it('a failed fetch KEEPS stale counts visible with the error + retry beside them', async () => {
    const { rerender } = await openCalendar();
    await rerender(baseProps({ availabilityError: 'unexpected error' }));
    // Grid stays (stale but visible) and the error is offered for retry.
    expect(screen.getByRole('button', { name: dayButtonName('2026-10-05', 2) })).toBeTruthy();
    expect(screen.getByText("Couldn't load recording dates.")).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Retry' })).toBeTruthy();
  });

  it('loaded + truly empty shows the distinct empty state', async () => {
    await openCalendar({ availabilityCounts: counts([]) });
    expect(screen.getByText('No recordings yet.')).toBeTruthy();
    expect(screen.queryByText("Couldn't load recording dates.")).toBeNull();
    expect(screen.queryByLabelText('Year')).toBeNull();
  });
});

// ── S-3 / PARENT-UI-1: viewport-aware popover placement ──────────────────
//
// The popover must be position:fixed (escaping .view-panel's
// overflow:hidden clipping), carry viewport-clamped inline geometry from
// computePopoverPlacement, and re-clamp when the window resizes WHILE
// OPEN. jsdom cannot lay out — the placement MATH is pinned in
// utils/popoverGeometry.test.ts; these pin the component integration.

describe('RecordingDateCalendar — viewport-aware placement (S-3)', () => {
  it('the open popover is fixed-positioned with clamped inline geometry and a placement marker', async () => {
    await openCalendar();
    const popover = screen.getByRole('dialog') as HTMLElement;
    expect(popover.classList.contains('calendar-popover')).toBe(true);
    // Fixed positioning is what escapes the clipping ancestor.
    expect(popover.style.position).toBe('fixed');
    // Inline geometry from computePopoverPlacement (jsdom: 1024x768,
    // trigger rect all zeros → below, clamped to the left margin).
    expect(popover.style.left).not.toBe('');
    expect(popover.style.top).not.toBe('');
    expect(popover.style.maxHeight).not.toBe('');
    expect(popover.dataset.placement).toBe('below');
  });

  it('re-clamps when the window is resized while the popover is open', async () => {
    const geometry = await import('../utils/popoverGeometry');
    const spy = vi.spyOn(geometry, 'computePopoverPlacement');
    try {
      await openCalendar();
      const callsAfterOpen = spy.mock.calls.length;
      expect(callsAfterOpen).toBeGreaterThan(0);

      await fireEvent(window, new Event('resize'));
      expect(spy.mock.calls.length).toBeGreaterThan(callsAfterOpen);

      // Re-clamping actually updates the inline geometry (the clamp
      // follows the new viewport, not the one at open time). jsdom lays
      // nothing out (trigger rect 0, natural height 0 → always "below"),
      // so the viewport change shows up in maxHeight: 768→300 shrinks the
      // below-space from 750 to 300-12-6=282.
      const popover = screen.getByRole('dialog') as HTMLElement;
      const firstMaxHeight = popover.style.maxHeight;
      window.innerWidth = 360;
      window.innerHeight = 300;
      await fireEvent(window, new Event('resize'));
      expect(spy.mock.calls.at(-1)?.[2]).toEqual({ width: 360, height: 300 });
      expect(popover.style.maxHeight).not.toBe(firstMaxHeight);
      expect(popover.style.maxHeight).toBe('282px');
    } finally {
      spy.mockRestore();
      window.innerWidth = 1024;
      window.innerHeight = 768;
    }
  });

  it('month navigation re-clamps (the grid can grow a row)', async () => {
    const geometry = await import('../utils/popoverGeometry');
    const spy = vi.spyOn(geometry, 'computePopoverPlacement');
    try {
      // 2025-11 (4 weeks + lead) → 2026-10 (spans 6 rows): taller grid.
      await openCalendar();
      const callsAfterOpen = spy.mock.calls.length;
      const year = screen.getByLabelText('Year') as HTMLSelectElement;
      await fireEvent.change(year, { target: { value: '2025' } });
      expect(spy.mock.calls.length).toBeGreaterThan(callsAfterOpen);
    } finally {
      spy.mockRestore();
    }
  });
});
