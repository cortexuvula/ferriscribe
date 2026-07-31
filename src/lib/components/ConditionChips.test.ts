// @vitest-environment jsdom
/**
 * ConditionChips — component-level render tests.
 *
 * The default vitest environment is "node" (see vitest.config.ts), which has
 * no DOM. These tests exercise the full component via @testing-library/svelte,
 * so the file-level pragma above switches this suite to jsdom.
 *
 * The component (ConditionChips.svelte) is "sync-aware": on mount it calls
 * `listConditionChips()` to load chips from the backend; while that promise is
 * pending (or if it resolves empty / rejects) it falls back to a built-in
 * `DEFAULT_CONDITIONS` list. Add/remove actions call
 * `addConditionChip` / `removeConditionChip` and replace the chip list with
 * whatever the backend returns. Errors are caught and logged (graceful
 * degradation) so the UI never crashes.
 *
 * Markup facts these tests rely on (kept in sync with the component):
 *   - The container is `<div class="condition-chips" role="group"
 *     aria-label="Common conditions quick-add">`.
 *   - Each condition renders as a `<button class="condition-chip">` whose text
 *     is the condition name.
 *   - Each has a sibling remove button `<button class="chip-remove">×</button>`
 *     with `aria-label="Remove {condition}"`.
 *   - The "add" affordance is a `<button class="chip-add">+</button>` that, when
 *     clicked, swaps in an `<input class="chip-input" placeholder="Condition name…">`.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/svelte';

// ---------------------------------------------------------------------------
// Mock the conditions API BEFORE importing the component. The component calls
// these on mount and on user action; we control their behavior to simulate the
// backend (success, empty, rejection).
// ---------------------------------------------------------------------------
const mockListConditionChips = vi.fn();
const mockAddConditionChip = vi.fn();
const mockRemoveConditionChip = vi.fn();
const mockReorderConditionChips = vi.fn();

vi.mock('../api/conditions', () => ({
  listConditionChips: (...args: unknown[]) => mockListConditionChips(...(args as [])),
  addConditionChip: (...args: unknown[]) => mockAddConditionChip(...(args as [string])),
  removeConditionChip: (...args: unknown[]) => mockRemoveConditionChip(...(args as [string])),
  reorderConditionChips: (...args: unknown[]) => mockReorderConditionChips(...(args as [string[]])),
}));

// The component now subscribes to a Tauri SSE event on mount. Mock the Tauri
// event + core APIs so importing/calling them doesn't fail under jsdom.
const mockInvoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...(args as [string, unknown?])),
}));

const mockListen = vi.fn();
vi.mock('@tauri-apps/api/event', () => ({
  listen: (...args: unknown[]) =>
    mockListen(...(args as [string, (e: unknown) => void])),
}));

// Import AFTER mocks are registered.
import ConditionChips from './ConditionChips.svelte';

// The exact default list baked into the component. Asserting against a known
// member (not the whole array) keeps the test robust to ordering tweaks while
// still proving the fallback list is rendered.
const DEFAULT_CONDITIONS = [
  'Hypertension',
  'Type 2 diabetes',
  'Hyperlipidemia',
  'Asthma',
  'COPD',
  'Hypothyroidism',
  'Atrial fibrillation',
  'Coronary artery disease',
  'CKD (chronic kidney disease)',
  'GERD',
  'Anxiety',
  'Depression',
  'Osteoarthritis',
  'Obesity',
  'Sleep apnea',
];

/** Helper: a ConditionChip-shaped object as returned by the API. sort_order is
 *  assigned from insertion order so each chip in a list gets a distinct index. */
function chip(text: string, sortOrder = 0) {
  return {
    id: `id-${text}`,
    text,
    updated_at: '2026-01-01T00:00:00Z',
    deleted_at: null,
    sort_order: sortOrder,
  };
}

beforeEach(() => {
  mockListConditionChips.mockReset();
  mockAddConditionChip.mockReset();
  mockRemoveConditionChip.mockReset();
  mockReorderConditionChips.mockReset();
  // Default: backend returns an empty list → component shows defaults.
  mockListConditionChips.mockResolvedValue([]);
  mockAddConditionChip.mockResolvedValue([]);
  mockRemoveConditionChip.mockResolvedValue([]);
  mockReorderConditionChips.mockResolvedValue([]);

  // The component calls `listen('condition-chips-changed', …)` and
  // `invoke('subscribe_condition_chips')` on mount. Default both to no-ops so
  // mount doesn't reject and tests stay focused on the chip behavior.
  mockListen.mockReset();
  mockListen.mockResolvedValue(async () => {});
  mockInvoke.mockReset();
  mockInvoke.mockResolvedValue(undefined);

  // jsdom doesn't implement pointer capture methods. Polyfill them as no-ops
  // so the pointer-event-based DnD handlers don't throw.
  if (!Element.prototype.setPointerCapture) {
    Element.prototype.setPointerCapture = vi.fn();
  }
  if (!Element.prototype.releasePointerCapture) {
    Element.prototype.releasePointerCapture = vi.fn();
  }
});

// Vitest globals are not enabled in this project, so @testing-library/svelte's
// built-in auto-cleanup (which keys off a global `afterEach`) never registers.
// Unmount between tests to keep the shared document isolated.
afterEach(() => cleanup());

describe('ConditionChips — render & load', () => {
  it('renders default conditions while loading (before list resolves)', () => {
    // Never-resolving list promise keeps the component in the "loading" state
    // where `loaded` is false, so DEFAULT_CONDITIONS must be showing.
    mockListConditionChips.mockReturnValue(new Promise(() => {}));

    render(ConditionChips, { onAdd: () => {} });

    const group = screen.getByRole('group', { name: 'Common conditions quick-add' });
    expect(group).toBeTruthy();

    // The tray collapses to the first COLLAPSED_COUNT chips with a "+N more"
    // toggle for the rest. The first batch of default conditions should each
    // be present as a clickable chip.
    for (const condition of DEFAULT_CONDITIONS.slice(0, 6)) {
      expect(screen.getByRole('button', { name: condition })).toBeTruthy();
    }
    // The remaining defaults are behind the toggle, surfaced as "+9 more".
    expect(screen.getByRole('button', { name: '+9 more' })).toBeTruthy();
  });

  it('loads chips from backend on mount and renders custom chips', async () => {
    // Backend returns a non-empty list → once loaded, those take precedence
    // over DEFAULT_CONDITIONS.
    const custom = [chip('Plantar fasciitis'), chip('Migraine')];
    mockListConditionChips.mockResolvedValue(custom);

    render(ConditionChips, { onAdd: () => {} });

    // After the mount effect flushes, custom chips appear.
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Plantar fasciitis' })).toBeTruthy();
    });
    expect(screen.getByRole('button', { name: 'Migraine' })).toBeTruthy();

    // listConditionChips was called exactly once on mount.
    expect(mockListConditionChips).toHaveBeenCalledTimes(1);
  });

  it('falls back to defaults when backend list resolves empty', async () => {
    mockListConditionChips.mockResolvedValue([]);

    render(ConditionChips, { onAdd: () => {} });

    // After load completes with an empty list, defaults still show.
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Hypertension' })).toBeTruthy();
    });
    expect(mockListConditionChips).toHaveBeenCalledTimes(1);
  });
});

describe('ConditionChips — graceful degradation', () => {
  it('falls back to defaults when listConditionChips rejects (no crash)', async () => {
    // Suppress the expected console.error noise from the component's catch.
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
    mockListConditionChips.mockRejectedValue(new Error('IPC failed'));

    render(ConditionChips, { onAdd: () => {} });

    // The default list renders synchronously, but the rejection is consumed by
    // the component's `catch` only on a later microtask. waitFor the
    // console.error call so we assert *after* onMount has settled the rejected
    // promise — proving the component caught the error and logged it.
    await waitFor(() => {
      expect(spy).toHaveBeenCalledWith('Failed to load condition chips:', expect.any(Error));
    });

    // Component did not throw; defaults render despite the failure.
    expect(screen.getByRole('button', { name: 'Type 2 diabetes' })).toBeTruthy();
    spy.mockRestore();
  });
});

describe('ConditionChips — add flow', () => {
  it('calls addConditionChip with the trimmed text when adding a chip', async () => {
    const updated = [chip('Hypertension'), chip('Gout')];
    mockAddConditionChip.mockResolvedValue(updated);
    // Start with a non-empty backend list so we're not relying on defaults.
    mockListConditionChips.mockResolvedValue([chip('Hypertension')]);

    render(ConditionChips, { onAdd: () => {} });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Hypertension' })).toBeTruthy();
    });

    // Click the "+" add button to reveal the input.
    const addBtn = screen.getByRole('button', { name: '+' });
    await fireEvent.click(addBtn);
    await tick();

    // Type a new condition and submit with Enter.
    const input = await screen.findByPlaceholderText('Condition name…');
    await fireEvent.input(input, { target: { value: '  Gout  ' } });
    await fireEvent.keyDown(input, { key: 'Enter' });

    await waitFor(() => {
      expect(mockAddConditionChip).toHaveBeenCalledTimes(1);
    });
    // The component trims before calling.
    expect(mockAddConditionChip).toHaveBeenCalledWith('Gout');

    // The returned list replaces the chips, so "Gout" now shows.
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Gout' })).toBeTruthy();
    });
  });

  it('does not call addConditionChip for empty/whitespace input', async () => {
    mockListConditionChips.mockResolvedValue([chip('Hypertension')]);

    render(ConditionChips, { onAdd: () => {} });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Hypertension' })).toBeTruthy();
    });

    await fireEvent.click(screen.getByRole('button', { name: '+' }));
    await tick();

    const input = await screen.findByPlaceholderText('Condition name…');
    await fireEvent.input(input, { target: { value: '   ' } });
    await fireEvent.keyDown(input, { key: 'Enter' });

    await tick();
    expect(mockAddConditionChip).not.toHaveBeenCalled();
  });

  it('does not call addConditionChip for a duplicate condition (case-insensitive)', async () => {
    mockListConditionChips.mockResolvedValue([chip('Hypertension')]);

    render(ConditionChips, { onAdd: () => {} });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Hypertension' })).toBeTruthy();
    });

    await fireEvent.click(screen.getByRole('button', { name: '+' }));
    await tick();

    const input = await screen.findByPlaceholderText('Condition name…');
    await fireEvent.input(input, { target: { value: 'hypertension' } });
    await fireEvent.keyDown(input, { key: 'Enter' });

    await tick();
    // Dedup guard short-circuits before the backend call.
    expect(mockAddConditionChip).not.toHaveBeenCalled();
  });
});

describe('ConditionChips — remove flow', () => {
  it('calls removeConditionChip when the × remove button is clicked', async () => {
    const remaining = [chip('Hypertension')];
    mockRemoveConditionChip.mockResolvedValue(remaining);
    mockListConditionChips.mockResolvedValue([chip('Hypertension'), chip('Asthma')]);

    render(ConditionChips, { onAdd: () => {} });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Asthma' })).toBeTruthy();
    });

    // Click the remove button for "Asthma" (aria-label="Remove Asthma preset").
    const removeBtn = screen.getByRole('button', { name: 'Remove Asthma preset' });
    await fireEvent.click(removeBtn);

    await waitFor(() => {
      expect(mockRemoveConditionChip).toHaveBeenCalledTimes(1);
    });
    expect(mockRemoveConditionChip).toHaveBeenCalledWith('Asthma');

    // Backend returned only Hypertension, so Asthma disappears.
    await waitFor(() => {
      expect(screen.queryByRole('button', { name: 'Asthma' })).toBeNull();
    });
    expect(screen.getByRole('button', { name: 'Hypertension' })).toBeTruthy();
  });

  it('keeps showing existing chips when removeConditionChip rejects (no crash)', async () => {
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
    mockRemoveConditionChip.mockRejectedValue(new Error('IPC failed'));
    mockListConditionChips.mockResolvedValue([chip('Hypertension'), chip('Asthma')]);

    render(ConditionChips, { onAdd: () => {} });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Asthma' })).toBeTruthy();
    });

    const removeBtn = screen.getByRole('button', { name: 'Remove Asthma preset' });
    await fireEvent.click(removeBtn);

    await waitFor(() => {
      expect(mockRemoveConditionChip).toHaveBeenCalledTimes(1);
    });

    // Chip is still present because the remove failed (graceful degradation).
    expect(screen.getByRole('button', { name: 'Asthma' })).toBeTruthy();
    expect(spy).toHaveBeenCalled();
    spy.mockRestore();
  });
});

describe('ConditionChips — drag-and-drop reorder', () => {
  it('renders chips with data-index for pointer-event DnD support', async () => {
    mockListConditionChips.mockResolvedValue([
      chip('Alpha', 0),
      chip('Beta', 1),
    ]);

    render(ConditionChips, { onAdd: () => {} });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Beta' })).toBeTruthy();
    });

    // Verify the chip wrappers have data-index attributes (used by pointer
    // event hit-testing in the DnD handler).
    const chipWrappers = document.querySelectorAll('[data-index]');
    expect(chipWrappers.length).toBe(2);
    expect(chipWrappers[0].getAttribute('data-index')).toBe('0');
    expect(chipWrappers[1].getAttribute('data-index')).toBe('1');
  });

  it('does not crash when clicking chips (pointer events do not interfere with click)', async () => {
    mockListConditionChips.mockResolvedValue([chip('Hypertension', 0)]);
    const onAdd = vi.fn();

    render(ConditionChips, { props: { onAdd } });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Hypertension' })).toBeTruthy();
    });

    // A simple click (no drag movement) should call onAdd.
    await fireEvent.click(screen.getByRole('button', { name: 'Hypertension' }));
    expect(onAdd).toHaveBeenCalledWith('Hypertension');
  });
});

describe('ConditionChips — realtime SSE sync', () => {
  it('subscribes to the condition-chips-changed event and the backend on mount', async () => {
    // Capture the handler passed to `listen` so we can simulate a server push.
    let capturedHandler: ((e: unknown) => void) | null = null;
    mockListen.mockImplementation(async (_eventName: string, handler: (e: unknown) => void) => {
      capturedHandler = handler;
      return async () => {};
    });
    mockListConditionChips.mockResolvedValue([chip('Hypertension')]);

    render(ConditionChips, { onAdd: () => {} });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Hypertension' })).toBeTruthy();
    });

    // The component registers a listener for the SSE event …
    await waitFor(() => {
      expect(mockListen).toHaveBeenCalledWith('condition-chips-changed', expect.any(Function));
    });
    // … and starts the backend SSE subscription task.
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('subscribe_condition_chips');
    });

    // Simulate a server push: a second chip now exists on the server. Firing
    // the captured handler should trigger a refreshChips() that pulls it in.
    mockListConditionChips.mockResolvedValue([chip('Hypertension'), chip('Asthma')]);
    expect(capturedHandler).not.toBeNull();
    capturedHandler!({ payload: null });

    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Asthma' })).toBeTruthy();
    });
  });
});

describe('ConditionChips — selection state (toggle)', () => {
  it('marks a chip active and calls onRemove when its text is in selectedConditions', async () => {
    mockListConditionChips.mockResolvedValue([chip('Hypertension', 0), chip('Asthma', 1)]);
    const onAdd = vi.fn();
    const onRemove = vi.fn();

    render(ConditionChips, {
      props: { onAdd, onRemove, selectedConditions: 'Hypertension\n' },
    });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Hypertension' })).toBeTruthy();
    });

    // Hypertension matches a line → active (✓ prefix, aria-pressed true).
    const htnBtn = screen.getByRole('button', { name: 'Hypertension' });
    expect(htnBtn.getAttribute('aria-pressed')).toBe('true');
    expect(htnBtn.textContent ?? '').toContain('✓');
    expect(htnBtn.closest('.condition-chip-wrapper')?.classList.contains('selected')).toBe(true);

    // Clicking the active chip removes it (toggles off).
    await fireEvent.click(htnBtn);
    expect(onRemove).toHaveBeenCalledWith('Hypertension');
    expect(onAdd).not.toHaveBeenCalled();
  });

  it('keeps a chip inactive and calls onAdd when its text is NOT in selectedConditions', async () => {
    mockListConditionChips.mockResolvedValue([chip('Hypertension', 0), chip('Asthma', 1)]);
    const onAdd = vi.fn();
    const onRemove = vi.fn();

    render(ConditionChips, {
      props: { onAdd, onRemove, selectedConditions: 'Hypertension\n' },
    });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Asthma' })).toBeTruthy();
    });

    // Asthma is not in the list → inactive.
    const asthmaBtn = screen.getByRole('button', { name: 'Asthma' });
    expect(asthmaBtn.getAttribute('aria-pressed')).toBe('false');
    expect(asthmaBtn.textContent ?? '').not.toContain('✓');
    expect(asthmaBtn.closest('.condition-chip-wrapper')?.classList.contains('selected')).toBe(false);

    // Clicking the inactive chip adds it (toggles on).
    await fireEvent.click(asthmaBtn);
    expect(onAdd).toHaveBeenCalledWith('Asthma');
    expect(onRemove).not.toHaveBeenCalled();
  });

  it('matches case-insensitively and ignores surrounding whitespace', async () => {
    mockListConditionChips.mockResolvedValue([chip('Hypertension')]);
    const onRemove = vi.fn();

    render(ConditionChips, {
      props: { onAdd: () => {}, onRemove, selectedConditions: '  hypertension  \n' },
    });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Hypertension' })).toBeTruthy();
    });

    const htnBtn = screen.getByRole('button', { name: 'Hypertension' });
    expect(htnBtn.getAttribute('aria-pressed')).toBe('true');
  });

  it('treats an absent selectedConditions as no active chips (back-compat)', async () => {
    mockListConditionChips.mockResolvedValue([chip('Hypertension')]);
    const onAdd = vi.fn();

    render(ConditionChips, { props: { onAdd } });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Hypertension' })).toBeTruthy();
    });

    const htnBtn = screen.getByRole('button', { name: 'Hypertension' });
    expect(htnBtn.getAttribute('aria-pressed')).toBe('false');
    expect(htnBtn.textContent ?? '').not.toContain('✓');

    // Click still routes to onAdd (original add-only behavior).
    await fireEvent.click(htnBtn);
    expect(onAdd).toHaveBeenCalledWith('Hypertension');
  });
});

describe('ConditionChips — collapsible tray', () => {
  it('collapses to the first 6 chips with a "+N more" toggle, then expands on click', async () => {
    // 8 chips → 6 visible, "+2 more".
    const eight = Array.from({ length: 8 }, (_, i) => chip(`Cond${i + 1}`, i));
    mockListConditionChips.mockResolvedValue(eight);

    render(ConditionChips, { onAdd: () => {} });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Cond1' })).toBeTruthy();
    });

    // First six are visible; the rest are behind the toggle.
    for (let i = 1; i <= 6; i++) {
      expect(screen.getByRole('button', { name: `Cond${i}` })).toBeTruthy();
    }
    expect(screen.queryByRole('button', { name: 'Cond7' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Cond8' })).toBeNull();
    expect(screen.getByRole('button', { name: '+2 more' })).toBeTruthy();

    // Expand → all eight visible, toggle now reads "Show less".
    await fireEvent.click(screen.getByRole('button', { name: '+2 more' }));
    await tick();
    expect(screen.getByRole('button', { name: 'Cond7' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Cond8' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Show less' })).toBeTruthy();

    // Collapse again → back to six.
    await fireEvent.click(screen.getByRole('button', { name: 'Show less' }));
    await tick();
    expect(screen.queryByRole('button', { name: 'Cond7' })).toBeNull();
    expect(screen.getByRole('button', { name: '+2 more' })).toBeTruthy();
  });

  it('renders all chips inline (no toggle) when count ≤ COLLAPSED_COUNT', async () => {
    const four = Array.from({ length: 4 }, (_, i) => chip(`Cond${i + 1}`, i));
    mockListConditionChips.mockResolvedValue(four);

    render(ConditionChips, { onAdd: () => {} });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Cond4' })).toBeTruthy();
    });

    for (let i = 1; i <= 4; i++) {
      expect(screen.getByRole('button', { name: `Cond${i}` })).toBeTruthy();
    }
    // No collapse toggle should be present.
    expect(screen.queryByRole('button', { name: /more/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /show less/i })).toBeNull();
  });
});
