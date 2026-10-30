<script lang="ts">
  import { fullLocalDateFromKey } from '../utils/recordingDates';
  import {
    computePopoverPlacement,
    type PopoverPlacement,
  } from '../utils/popoverGeometry';

  /// Date-filter calendar for the Active recordings view
  /// (recording-date-calendar, batch 3).
  ///
  /// A labelled, NONMODAL popover beside the search bar. It renders ONLY
  /// the populated local days from the store's availability counts as
  /// ordinary buttons (full-date + count accessible names, aria-pressed +
  /// a ✓ cue for the selected day) — every other position is inert
  /// aria-hidden spacing. Month/year navigation is restricted to populated
  /// months (prev/next jump to the adjacent populated month; the selects
  /// offer only populated years/months), so an unpopulated view can never
  /// appear. Deliberately NOT a role=grid: the day buttons are plain
  /// buttons in a labelled dialog, without pretend-grid keyboard semantics.
  ///
  /// Placement (S-3): the popover is position:fixed with geometry computed
  /// from the AVAILABLE viewport — it escapes clipping ancestors (the
  /// view panel's overflow:hidden), clamps its horizontal bounds, flips
  /// above the trigger, or clamps to the roomier side and scrolls inside
  /// when neither fits. Re-clamped on window resize WHILE OPEN and when
  /// the rendered month changes the grid height.

  interface Props {
    /// Active-recording counts per local day key ('YYYY-MM-DD') — the
    /// store's whole-DB availability, independent of search/date filters.
    availabilityCounts: Map<string, number>;
    availabilityLoading?: boolean;
    availabilityError?: string | null;
    availabilityLoaded?: boolean;
    /// Currently applied local day key, or null (all dates).
    selectedDate?: string | null;
    /// A populated day was picked.
    onSelectDate: (key: string) => void;
    /// Fired on open and on Retry — the store refetches availability so a
    /// timezone change since the last fetch regroups days correctly.
    onRefreshAvailability: () => void;
  }

  const {
    availabilityCounts,
    availabilityLoading = false,
    availabilityError = null,
    availabilityLoaded = false,
    selectedDate = null,
    onSelectDate,
    onRefreshAvailability,
  }: Props = $props();

  const uid = $props.id();
  const popoverId = `${uid}-popover`;
  const titleId = `${uid}-title`;

  const POPOVER_PREFERRED_WIDTH = 322;

  let open = $state(false);
  let viewYear = $state(1970);
  let viewMonth = $state(0);
  let popoverEl = $state<HTMLElement | null>(null);
  let triggerEl = $state<HTMLButtonElement | null>(null);
  /// Current viewport-computed fixed geometry (null while closed).
  let placement = $state<PopoverPlacement | null>(null);
  /// True once focus has entered the popover this opening — drives the
  /// "focused day disappeared under a refresh" recovery (a focus loss we
  /// did not cause ourselves must be repaired, not ignored).
  let hadInsideFocus = false;

  function pad2(n: number): string {
    return String(n).padStart(2, '0');
  }
  function monthKeyOf(year: number, month: number): string {
    return `${year}-${pad2(month + 1)}`;
  }
  function applyMonthKey(key: string): void {
    viewYear = Number(key.slice(0, 4));
    viewMonth = Number(key.slice(5, 7)) - 1;
  }

  /// All populated 'YYYY-MM' keys, sorted ascending (lexicographic ==
  /// chronological for zero-padded keys). Plain arrays, deduped inline —
  /// the populations are tiny (months with recordings) and the derived
  /// already re-runs when the counts prop changes.
  const sortedMonthKeys = $derived.by(() => {
    const seen: string[] = [];
    for (const k of availabilityCounts.keys()) {
      const m = k.slice(0, 7);
      if (!seen.includes(m)) seen.push(m);
    }
    return seen.sort();
  });
  const hasData = $derived(availabilityCounts.size > 0);
  const viewKey = $derived(monthKeyOf(viewYear, viewMonth));

  const prevKey = $derived.by(() => {
    const i = sortedMonthKeys.indexOf(viewKey);
    return i > 0 ? sortedMonthKeys[i - 1] : null;
  });
  const nextKey = $derived.by(() => {
    const i = sortedMonthKeys.indexOf(viewKey);
    return i >= 0 && i < sortedMonthKeys.length - 1 ? sortedMonthKeys[i + 1] : null;
  });

  const populatedYears = $derived.by(() => {
    const ys: number[] = [];
    for (const k of sortedMonthKeys) {
      const y = Number(k.slice(0, 4));
      if (!ys.includes(y)) ys.push(y);
    }
    return ys;
  });
  /// Populated months of the CURRENT view year (month indexes 0-11).
  const monthsOfYear = $derived.by(() => {
    const ms: number[] = [];
    for (const k of sortedMonthKeys) {
      if (k.startsWith(`${viewYear}-`)) ms.push(Number(k.slice(5, 7)) - 1);
    }
    return ms;
  });

  function monthName(m: number): string {
    return new Date(2026, m, 1).toLocaleDateString(undefined, { month: 'long' });
  }
  function dayLabel(key: string, count: number): string {
    return `${fullLocalDateFromKey(key)}, ${count} recording${count === 1 ? '' : 's'}`;
  }

  /// 2026-01-04 is a Sunday — one fixed week giving locale-correct short
  /// weekday names in Sun..Sat (the grid's column) order.
  const WEEKDAYS: string[] = Array.from({ length: 7 }, (_, i) =>
    new Date(2026, 0, 4 + i).toLocaleDateString(undefined, { weekday: 'short' }),
  );

  interface DayCell {
    key: string | null;
    day: number;
    count: number;
    selected: boolean;
  }

  const cells = $derived.by(() => {
    const out: DayCell[] = [];
    const lead = new Date(viewYear, viewMonth, 1).getDay();
    for (let i = 0; i < lead; i++) out.push({ key: null, day: 0, count: 0, selected: false });
    const days = new Date(viewYear, viewMonth + 1, 0).getDate();
    for (let d = 1; d <= days; d++) {
      const key = `${viewYear}-${pad2(viewMonth + 1)}-${pad2(d)}`;
      const n = availabilityCounts.get(key) ?? 0;
      out.push({ key: n > 0 ? key : null, day: d, count: n, selected: key === selectedDate });
    }
    while (out.length % 7 !== 0) out.push({ key: null, day: 0, count: 0, selected: false });
    return out;
  });

  /// The month the calendar opens on: the selected day's month when that
  /// day is still populated, else the newest populated month. Never an
  /// unpopulated month (the selects only contain populated choices).
  function initialMonthKey(): string | null {
    if (selectedDate && availabilityCounts.has(selectedDate)) {
      return selectedDate.slice(0, 7);
    }
    return sortedMonthKeys.length > 0 ? sortedMonthKeys[sortedMonthKeys.length - 1] : null;
  }

  function openPopover(): void {
    open = true;
    hadInsideFocus = false;
    // Re-derive availability on every open: the counts may have been
    // grouped under a different timezone than the browser now runs.
    onRefreshAvailability();
    const target = initialMonthKey();
    if (target) applyMonthKey(target);
  }

  function closePopover(restoreFocusToTrigger: boolean): void {
    open = false;
    hadInsideFocus = false;
    placement = null;
    if (restoreFocusToTrigger) triggerEl?.focus();
  }

  function selectDay(key: string): void {
    onSelectDate(key);
    closePopover(true);
  }

  function focusInitialDay(): void {
    const el = popoverEl;
    if (!el) return;
    const target =
      (selectedDate
        ? el.querySelector<HTMLButtonElement>(`button[data-day="${selectedDate}"]`)
        : null) ??
      el.querySelector<HTMLButtonElement>('button[data-day]') ??
      el.querySelector<HTMLButtonElement>('button:not([disabled])');
    target?.focus();
  }

  function handleWindowKeydown(e: KeyboardEvent): void {
    if (!open || e.key !== 'Escape') return;
    e.preventDefault();
    closePopover(true);
  }

  function handleWindowClick(e: MouseEvent): void {
    if (!open) return;
    const t = e.target;
    if (!(t instanceof Node)) return;
    if (popoverEl?.contains(t) || triggerEl?.contains(t)) return;
    closePopover(false);
  }

  /// Recompute the popover's fixed-position geometry from the CURRENT
  /// trigger rect, popover content height, and AVAILABLE viewport (see
  /// utils/popoverGeometry for the flip/clamp rules). Runs on open, on
  /// window resize WHILE OPEN (the window can shrink under an open
  /// popover), and whenever the rendered content changes height (counts
  /// landing, month navigation adding/removing a week row).
  function reposition(): void {
    const pop = popoverEl;
    const trig = triggerEl;
    if (!open || !pop || !trig) return;
    const rect = trig.getBoundingClientRect();
    placement = computePopoverPlacement(
      { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom },
      pop.scrollHeight,
      { width: window.innerWidth, height: window.innerHeight },
      POPOVER_PREFERRED_WIDTH,
    );
  }

  function handleWindowResize(): void {
    if (open) reposition();
  }

  function onYearChange(e: Event): void {
    const year = Number((e.currentTarget as HTMLSelectElement).value);
    viewYear = year;
    if (!sortedMonthKeys.includes(monthKeyOf(year, viewMonth))) {
      const k = sortedMonthKeys.find((key) => key.startsWith(`${year}-`));
      if (k) applyMonthKey(k);
    }
  }

  function onMonthChange(e: Event): void {
    applyMonthKey(`${viewYear}-${pad2(Number((e.currentTarget as HTMLSelectElement).value) + 1)}`);
  }

  // Availability refreshes and month navigation both change the rendered
  // day set while the popover is open. Keep the view month populated (jump
  // when its days all emptied), and recover focus whenever the focused day
  // button disappeared — the selected day, else the first available day,
  // else the first enabled control.
  $effect(() => {
    if (!open) return;
    void availabilityCounts;
    const el = popoverEl;
    if (!el) return;
    if (hasData && !sortedMonthKeys.includes(viewKey)) {
      const next = sortedMonthKeys.find((k) => k >= viewKey) ?? sortedMonthKeys[sortedMonthKeys.length - 1];
      applyMonthKey(next);
    }
    const active = document.activeElement;
    if (hadInsideFocus && active instanceof Node && el.contains(active)) return;
    focusInitialDay();
  });

  // Placement: compute post-render (scrollHeight is the natural,
  // unclamped content height), and recompute whenever the rendered
  // content or month changes the height. Window resizes are covered by
  // handleWindowResize (Svelte cannot observe innerWidth reactively).
  $effect(() => {
    if (!open) {
      placement = null;
      return;
    }
    void popoverEl;
    void triggerEl;
    void availabilityCounts;
    void viewKey;
    void hasData;
    reposition();
  });
</script>

<svelte:window
  onkeydown={handleWindowKeydown}
  onclick={handleWindowClick}
  onresize={handleWindowResize}
/>

<span class="calendar-anchor">
  <button
    type="button"
    bind:this={triggerEl}
    class="btn-calendar-trigger"
    aria-expanded={open}
    aria-haspopup="dialog"
    aria-controls={popoverId}
    onclick={() => (open ? closePopover(true) : openPopover())}
  >
    <span aria-hidden="true">📅</span>
    Choose recording date
  </button>

  {#if open}
    <div
      bind:this={popoverEl}
      id={popoverId}
      role="dialog"
      aria-modal="false"
      aria-labelledby={titleId}
      class="calendar-popover"
      data-placement={placement?.placement}
      style={placement
        ? `position:fixed;left:${placement.left}px;max-height:${placement.maxHeight}px;top:${placement.top}px;width:${placement.width}px;`
        : ''}
      onfocusin={() => (hadInsideFocus = true)}
    >
      <div class="popover-head">
        <h2 id={titleId} class="popover-title">Choose recording date</h2>
        <button
          type="button"
          class="btn-close"
          aria-label="Close"
          onclick={() => closePopover(true)}
        >
          <span aria-hidden="true">×</span>
        </button>
      </div>

      {#if hasData}
        <div class="month-controls">
          <button
            type="button"
            class="btn-month-nav"
            aria-label="Previous month with recordings"
            disabled={prevKey === null}
            onclick={() => prevKey && applyMonthKey(prevKey)}
          >
            <span aria-hidden="true">‹</span>
          </button>
          <select aria-label="Year" class="select-year" value={String(viewYear)} onchange={onYearChange}>
            {#each populatedYears as y (y)}
              <option value={String(y)}>{y}</option>
            {/each}
          </select>
          <select aria-label="Month" class="select-month" value={String(viewMonth)} onchange={onMonthChange}>
            {#each monthsOfYear as m (m)}
              <option value={String(m)}>{monthName(m)}</option>
            {/each}
          </select>
          <button
            type="button"
            class="btn-month-nav"
            aria-label="Next month with recordings"
            disabled={nextKey === null}
            onclick={() => nextKey && applyMonthKey(nextKey)}
          >
            <span aria-hidden="true">›</span>
          </button>
        </div>

        <div class="weekday-row" aria-hidden="true">
          {#each WEEKDAYS as wd (wd)}
            <span class="weekday">{wd}</span>
          {/each}
        </div>
        <div class="calendar-grid">
          {#each cells as cell, i (i)}
            {#if cell.key}
              {@const key = cell.key}
              <button
                type="button"
                class="day"
                class:day-selected={cell.selected}
                data-day={key}
                aria-pressed={cell.selected ? 'true' : 'false'}
                aria-label={dayLabel(key, cell.count)}
                onclick={() => selectDay(key)}
              >
                <span class="day-num">
                  {#if cell.selected}<span class="sel-mark" aria-hidden="true">✓</span>{/if}{cell.day}
                </span>
                {#if cell.count > 0}<span class="day-count">{cell.count}</span>{/if}
              </button>
            {:else}
              <div class="day blank" aria-hidden="true"></div>
            {/if}
          {/each}
        </div>
      {/if}

      <div class="calendar-status">
        {#if availabilityError}
          <p class="status-error" role="alert">Couldn't load recording dates.</p>
          <button type="button" class="btn-retry" onclick={onRefreshAvailability}>Retry</button>
        {:else if !hasData && availabilityLoading}
          <p role="status">Loading recording dates…</p>
        {:else if hasData && availabilityLoading}
          <p role="status" class="status-subtle">Updating…</p>
        {:else if !hasData && availabilityLoaded}
          <p>No recordings yet.</p>
        {/if}
      </div>
    </div>
  {/if}
</span>

<style>
  .calendar-anchor {
    display: inline-flex;
    flex: 0 0 auto;
  }

  .btn-calendar-trigger {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 12px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    cursor: pointer;
    white-space: nowrap;
    transition: background-color 0.15s ease, border-color 0.15s ease, color 0.15s ease;
  }

  .btn-calendar-trigger:hover {
    background-color: var(--bg-hover);
    border-color: var(--accent);
    color: var(--text-primary);
  }

  .btn-calendar-trigger:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  /* Viewport-computed geometry (position/size) arrives as INLINE styles
     from reposition() — fixed positioning escapes clipping ancestors
     (the view panel's overflow:hidden) and the computed placement never
     crosses the viewport edge; when neither side of the trigger fits the
     popover clamps to the roomier side and scrolls internally. */
  .calendar-popover {
    position: fixed;
    z-index: 60;
    overflow-y: auto;
    padding: 10px;
    background-color: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow-md);
  }

  .popover-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    margin-bottom: 8px;
  }

  .popover-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .btn-close {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    font-size: 16px;
    line-height: 1;
    color: var(--text-muted);
    background: transparent;
    border: none;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .btn-close:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  .month-controls {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-bottom: 6px;
  }

  .btn-month-nav {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 28px;
    height: 28px;
    font-size: 16px;
    line-height: 1;
    color: var(--text-secondary);
    background: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .btn-month-nav:hover:not(:disabled) {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .btn-month-nav:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .select-year,
  .select-month {
    flex: 1 1 0;
    min-height: 28px;
    padding: 2px 4px;
    font-size: 12px;
    color: var(--text-primary);
    background-color: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }

  .weekday-row,
  .calendar-grid {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 2px;
  }

  .weekday {
    padding: 2px 0;
    font-size: 10px;
    font-weight: 600;
    text-align: center;
    color: var(--text-muted);
  }

  .day {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    min-height: 34px;
    padding: 2px 1px;
    font-size: 12px;
    line-height: 1.2;
    color: var(--text-primary);
    background-color: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .day:hover {
    background-color: var(--bg-hover);
  }

  /* Selected day: aria-pressed PLUS a non-color cue — the ✓ mark and a
     bold weight + accent ring, so selection never rides on hue alone. */
  .day-selected {
    font-weight: 700;
    background-color: var(--accent-light);
    border-color: var(--accent);
  }

  .sel-mark {
    margin-right: 2px;
    color: var(--accent);
  }

  .day-count {
    font-size: 10px;
    color: var(--text-muted);
  }

  .day-selected .day-count {
    color: var(--text-secondary);
  }

  /* Focus is managed programmatically (open/recovery), so :focus — not
     only :focus-visible — must stay visibly indicated. */
  .calendar-popover button:focus,
  .calendar-popover select:focus {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }

  .calendar-status {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    min-height: 28px;
    padding-top: 6px;
    margin-top: 4px;
    border-top: 1px solid var(--border-light);
    font-size: 12px;
    color: var(--text-muted);
  }

  .status-error {
    color: var(--danger);
  }

  .status-subtle {
    font-style: italic;
  }

  .btn-retry {
    padding: 2px 12px;
    font-size: 12px;
    font-weight: 500;
    color: var(--accent);
    background-color: transparent;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .btn-retry:hover {
    background-color: var(--bg-hover);
    border-color: var(--accent);
  }
</style>
