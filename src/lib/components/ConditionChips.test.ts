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

    // Every default condition should be present as a clickable chip.
    for (const condition of DEFAULT_CONDITIONS) {
      // Multiple buttons share the condition name (chip label), so we grab the
      // role=name button which is the chip itself.
      expect(screen.getByRole('button', { name: condition })).toBeTruthy();
    }
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

    // Click the remove button for "Asthma" (aria-label="Remove Asthma").
    const removeBtn = screen.getByRole('button', { name: 'Remove Asthma' });
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

    const removeBtn = screen.getByRole('button', { name: 'Remove Asthma' });
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
  it('calls reorderConditionChips when chips are dragged to a new position', async () => {
    mockListConditionChips.mockResolvedValue([
      chip('Alpha', 0),
      chip('Beta', 1),
    ]);
    // Backend echoes back the new order after persisting.
    mockReorderConditionChips.mockResolvedValue([chip('Beta', 0), chip('Alpha', 1)]);

    render(ConditionChips, { onAdd: () => {} });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Beta' })).toBeTruthy();
    });

    // The chip wrapper divs only become draggable once `loaded` is true, which
    // happens after the (non-empty) backend list resolves. Wait for at least
    // two truly-draggable wrappers.
    await waitFor(() => {
      const wrappers = document.querySelectorAll('[draggable="true"]');
      expect(wrappers.length).toBeGreaterThanOrEqual(2);
    });
    const chipWrappers = document.querySelectorAll<HTMLElement>('[draggable="true"]');

    // Simulate dragging Alpha (index 0) onto Beta's position (index 1).
    // fireEvent.dragStart/dragOver/drop dispatch native DnD events on the
    // wrapper divs that carry ondragstart/ondragover/ondrop handlers.
    await fireEvent.dragStart(chipWrappers[0]);
    await fireEvent.dragOver(chipWrappers[1]);
    await fireEvent.drop(chipWrappers[1]);

    // The component splices index 0 → index 1, producing [Beta, Alpha], and
    // calls reorderConditionChips with that ordered ID list.
    await waitFor(() => {
      expect(mockReorderConditionChips).toHaveBeenCalledTimes(1);
    });
    expect(mockReorderConditionChips).toHaveBeenCalledWith(['id-Beta', 'id-Alpha']);
  });

  it('does not call reorderConditionChips when a chip is dropped on its own position', async () => {
    mockListConditionChips.mockResolvedValue([chip('Alpha', 0), chip('Beta', 1)]);

    render(ConditionChips, { onAdd: () => {} });
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Alpha' })).toBeTruthy();
    });

    await waitFor(() => {
      const wrappers = document.querySelectorAll('[draggable="true"]');
      expect(wrappers.length).toBeGreaterThanOrEqual(2);
    });
    const chipWrappers = document.querySelectorAll<HTMLElement>('[draggable="true"]');

    // Dragging a chip onto itself is a no-op.
    await fireEvent.dragStart(chipWrappers[0]);
    await fireEvent.dragOver(chipWrappers[0]);
    await fireEvent.drop(chipWrappers[0]);

    // Give any stray async work a chance to flush, then assert no reorder.
    await tick();
    await waitFor(() => {
      expect(mockReorderConditionChips).not.toHaveBeenCalled();
    });
  });
});
