// @vitest-environment jsdom
/**
 * DataManagement — component-level render tests for the Recording Retention
 * setting.
 *
 * The default vitest environment is "node" (see vitest.config.ts), which has
 * no DOM. These tests exercise the full section via @testing-library/svelte,
 * so the file-level pragma above switches this suite to jsdom.
 *
 * Mounting this section pulls in a heavy graph (three dialogs, vocabulary /
 * context-template / dictionary APIs, tauri plugin-dialog). The mocks below
 * keep the mount safe without asserting on their behaviour:
 *   - settings store is replaced with a plain state object + spied
 *     `updateField`, so these tests assert exactly what the retention
 *     select persists (`retention_days`: number | null).
 *   - toasts / contextTemplates stores are stubbed out (mount-path calls:
 *     `contextTemplates.load()` in onMount).
 *   - `@tauri-apps/api/core` is mocked with a command-aware `invoke` for the
 *     two mount-path commands (`get_vocabulary_count`, `user_dict_list`).
 *     Nothing else in the loaded graph imports other core exports.
 *   - The three dialogs stay closed (`open === false`), so their `{#if open}`
 *     interiors never render.
 *
 * Markup facts these tests rely on (kept in sync with the component):
 *   - The retention select is `<select id="retention-select">` labelled by
 *     `<label for="retention-select">`.
 *   - Options: 0 = "Never (keep forever)", 30/90/180/365 = "N days".
 *   - The hint span mentions the 30-day undo window.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { render, screen, fireEvent, cleanup } from '@testing-library/svelte';

// ---------------------------------------------------------------------------
// Mocks BEFORE importing the component (hoisted by vitest).
// ---------------------------------------------------------------------------
const mockUpdateField = vi.fn();

// The slice of AppConfig the section's template reads. Mutated per-test to
// control what the select renders before any user interaction. vi.hoisted
// runs before the module's imports, so the vi.mock factory below can close
// over it (factories execute while the component module graph loads).
const mockSettingsState = vi.hoisted(() => ({
  vocabulary_enabled: false,
  medical_dict_enabled: false,
  retention_days: null as number | null,
}));

vi.mock('../../../stores/settings.svelte', () => ({
  settings: {
    state: mockSettingsState,
    updateField: (...args: unknown[]) => mockUpdateField(...(args as [])),
  },
}));

vi.mock('../../../stores/toasts.svelte', () => ({
  toasts: { success: vi.fn(), error: vi.fn() },
}));

const mockCtxTemplatesLoad = vi.fn();
vi.mock('../../../stores/contextTemplates.svelte', () => ({
  contextTemplates: {
    list: [],
    load: (...args: unknown[]) => mockCtxTemplatesLoad(...(args as [])),
  },
}));

const mockInvoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => mockInvoke(...(args as [string, unknown?])),
}));

// Import AFTER mocks are registered.
import DataManagement from './DataManagement.svelte';

function renderSection() {
  render(DataManagement);
}

function getRetentionSelect(): HTMLSelectElement {
  return screen.getByLabelText('Automatically move recordings to trash when older than', {
    selector: 'select',
  }) as HTMLSelectElement;
}

beforeEach(() => {
  mockUpdateField.mockClear();
  mockCtxTemplatesLoad.mockClear();
  // Mount-path backend calls: counts resolve to empty, dict resolves to [].
  mockInvoke.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case 'get_vocabulary_count':
        return [0, 0];
      case 'user_dict_list':
        return [];
      default:
        return null;
    }
  });
  mockSettingsState.retention_days = null;
});

afterEach(cleanup);

describe('DataManagement — Recording Retention', () => {
  it('renders "Never (keep forever)" selected when retention_days is null (default)', async () => {
    renderSection();
    await tick();
    const select = getRetentionSelect();
    expect(select.value).toBe('0');
    expect(select.selectedOptions[0]?.textContent?.trim()).toBe('Never (keep forever)');
  });

  it('renders the configured window selected (90 → "90 days")', async () => {
    mockSettingsState.retention_days = 90;
    renderSection();
    await tick();
    const select = getRetentionSelect();
    expect(select.value).toBe('90');
    expect(select.selectedOptions[0]?.textContent?.trim()).toBe('90 days');
  });

  it('changing the select persists the window via updateField', () => {
    renderSection();
    fireEvent.change(getRetentionSelect(), { target: { value: '90' } });
    expect(mockUpdateField).toHaveBeenCalledWith('retention_days', 90);
    expect(mockUpdateField).toHaveBeenCalledTimes(1);
  });

  it('choosing "Never" persists null, not 0', () => {
    mockSettingsState.retention_days = 30;
    renderSection();
    fireEvent.change(getRetentionSelect(), { target: { value: '0' } });
    expect(mockUpdateField).toHaveBeenCalledWith('retention_days', null);
  });

  it('helper text documents the 30-day undo window', () => {
    renderSection();
    expect(
      screen.getByText(/30-day undo window before permanent deletion/),
    ).toBeTruthy();
    expect(screen.getByText(/Restoring a recording exempts it/)).toBeTruthy();
  });
});
