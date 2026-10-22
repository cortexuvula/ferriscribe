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
 *   - Options: 0 = "Never automatically move to Trash", 30/90/180/365 = "N days".
 *   - The Trash select controls a separate clock and confirms accelerated purge.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { tick } from 'svelte';
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/svelte';
import { confirmDialog } from '../../../stores/confirm.svelte';
import { confirmStore } from '../../../stores/confirm.svelte';
import ConfirmHost from '../../ConfirmHost.svelte';

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
  trash_retention_days: 30 as number | null | undefined,
}));

vi.mock('../../../stores/confirm.svelte', async (importOriginal) => ({
  ...await importOriginal<typeof import('../../../stores/confirm.svelte')>(),
  confirmDialog: vi.fn(async () => true),
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
import DataManagement from './RecordingRetention.svelte';

function renderSection() {
  render(DataManagement);
}

function getRetentionSelect(): HTMLSelectElement {
  return screen.getByLabelText('Move recordings to Trash when older than', {
    selector: 'select',
  }) as HTMLSelectElement;
}

beforeEach(() => {
  confirmStore.settle(false);
  mockUpdateField.mockClear();
  vi.mocked(confirmDialog).mockReset().mockResolvedValue(true);
  mockUpdateField.mockResolvedValue(undefined);
  mockCtxTemplatesLoad.mockClear();
  // Mount-path backend calls: counts resolve to empty, dict resolves to [].
  mockInvoke.mockReset();
  mockInvoke.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case 'get_vocabulary_count':
        return [0, 0];
      case 'user_dict_list':
        return [];
      case 'count_soft_deleted_older_than':
        return 3;
      default:
        return null;
    }
  });
  mockSettingsState.retention_days = null;
  mockSettingsState.trash_retention_days = 30;
});

afterEach(cleanup);

describe('DataManagement — Recording Retention', () => {
  it('renders "Never automatically move to Trash" selected when retention_days is null (default)', async () => {
    renderSection();
    await tick();
    const select = getRetentionSelect();
    expect(select.value).toBe('0');
    expect(select.selectedOptions[0]?.textContent?.trim()).toBe('Never automatically move to Trash');
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

  it('helper text explains irreversible deletion of the recording and its documents', () => {
    renderSection();
    expect(
      screen.getByText('When the window ends, cleanup permanently deletes the audio, transcript, SOAP notes and generated documents. They cannot be restored from Trash.'),
    ).toBeTruthy();
    expect(screen.getByText(/Restoring a recording exempts it/)).toBeTruthy();
    expect(screen.queryByText(/30-day undo window/)).toBeNull();
  });

  function getTrashSelect(): HTMLSelectElement {
    return screen.getByLabelText('Permanently delete from Trash after') as HTMLSelectElement;
  }

  it.each([undefined, null, 30])('defaults the separate Trash select to 30 for %s', (value) => {
    mockSettingsState.trash_retention_days = value;
    renderSection();
    const select = getTrashSelect();
    expect(select.id).toBe('trash-retention-select');
    expect(select.value).toBe('30');
    expect(select.selectedOptions[0].textContent).toBe('30 days (default)');
    expect(Array.from(select.options, option => option.value)).toEqual(['7', '14', '21', '30']);
  });

  it('requires confirmation naming the count before saving a shorter window', async () => {
    let resolve!: (ok: boolean) => void;
    vi.mocked(confirmDialog).mockReturnValue(new Promise(r => { resolve = r; }));
    renderSection();
    await fireEvent.change(getTrashSelect(), { target: { value: '7' } });
    await waitFor(() => expect(confirmDialog).toHaveBeenCalledWith(expect.objectContaining({
      title: 'Shorten Trash retention to 7 days?',
      message: 'This change affects recordings already in Trash and future recordings. 3 recordings already exceed the new limit and will be permanently deleted at the next cleanup. Cleanup runs about five minutes after launch, then once daily while FerriScribe is open. Their audio, transcripts, SOAP notes and generated documents cannot be restored from Trash afterward.',
      confirmLabel: 'Set 7-day window', cancelLabel: 'Keep 30 days', danger: true,
    })));
    expect(mockInvoke).toHaveBeenCalledWith('count_soft_deleted_older_than', { days: 7 });
    expect(mockUpdateField).not.toHaveBeenCalled();
    resolve(true);
    await waitFor(() => expect(mockUpdateField).toHaveBeenCalledWith('trash_retention_days', 7, 3));
  });

  it('cancel reverts the select without saving', async () => {
    vi.mocked(confirmDialog).mockResolvedValue(false);
    renderSection();
    await fireEvent.change(getTrashSelect(), { target: { value: '7' } });
    await waitFor(() => expect(confirmDialog).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(getTrashSelect().value).toBe('30'));
    expect(mockUpdateField).not.toHaveBeenCalled();
  });

  it('saves a shorter window with zero eligible recordings without a dialog', async () => {
    mockInvoke.mockResolvedValue(0);
    renderSection();
    await fireEvent.change(getTrashSelect(), { target: { value: '14' } });
    await waitFor(() => expect(mockUpdateField).toHaveBeenCalledWith('trash_retention_days', 14, 0));
    expect(confirmDialog).not.toHaveBeenCalled();
  });

  it('increasing the window needs neither a count nor confirmation', async () => {
    mockSettingsState.trash_retention_days = 7;
    renderSection();
    await fireEvent.change(getTrashSelect(), { target: { value: '21' } });
    await waitFor(() => expect(mockUpdateField).toHaveBeenCalledWith('trash_retention_days', 21, undefined));
    expect(confirmDialog).not.toHaveBeenCalled();
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('a failed count fails closed, reverts the selection and restores focus', async () => {
    mockInvoke.mockRejectedValue(new Error('unavailable'));
    renderSection();
    await fireEvent.change(getTrashSelect(), { target: { value: '7' } });
    await waitFor(() => expect(getTrashSelect().value).toBe('30'));
    await waitFor(() => expect(document.activeElement).toBe(getTrashSelect()));
    expect(mockUpdateField).not.toHaveBeenCalled();
  });

  it('groups both stages under one section and connects each select to its hint', () => {
    renderSection();
    expect(screen.getAllByRole('heading', { level: 3 })).toHaveLength(1);
    expect(screen.getByRole('heading', { name: 'Recording retention', level: 3 })).toBeTruthy();
    for (const name of ['Move to Trash', 'Permanent deletion']) {
      expect(screen.getByRole('heading', { name, level: 4 }).classList.contains('preference-heading')).toBe(true);
    }
    expect(screen.getByText('Choose when recordings move to Trash and how long they remain available to restore. Cleanup runs about five minutes after launch, then once daily while FerriScribe is open.')).toBeTruthy();
    for (const select of [getRetentionSelect(), getTrashSelect()]) {
      const id = select.getAttribute('aria-describedby');
      expect(id).toBeTruthy();
      expect(document.getElementById(id!)?.classList.contains('form-hint')).toBe(true);
    }
    expect(screen.queryByText(/Seven days is the shortest recovery window/)).toBeNull();
  });

  it('describes the seven-day safeguard with the themed warning Callout', () => {
    mockSettingsState.trash_retention_days = 7;
    renderSection();
    const warning = screen.getByText('Seven days is the shortest recovery window. Review Trash before saving: recordings already there may become eligible for permanent deletion.');
    expect(warning.classList.contains('warning')).toBe(true);
    const ids = getTrashSelect().getAttribute('aria-describedby')!.split(' ');
    expect(ids).toHaveLength(2);
    expect(ids.some(id => document.getElementById(id)?.contains(warning))).toBe(true);
  });

  it('confirms seven days even when no recordings currently exceed the new limit', async () => {
    mockInvoke.mockResolvedValue(0);
    vi.mocked(confirmDialog).mockImplementation(opts => confirmStore.confirm(opts));
    renderSection();
    render(ConfirmHost);
    const select = getTrashSelect();
    select.focus();
    await fireEvent.change(select, { target: { value: '7' } });
    const dialog = await screen.findByRole('alertdialog', { name: 'Shorten Trash retention to 7 days?' });
    expect(dialog.textContent).toContain('No recordings currently exceed the 7-day limit.');
    expect(dialog.textContent).toContain('This change affects recordings already in Trash and future recordings.');
    expect(dialog.textContent).toContain('Cleanup runs about five minutes after launch, then once daily while FerriScribe is open.');
    expect(mockUpdateField).not.toHaveBeenCalled();
    await fireEvent.click(screen.getByRole('button', { name: 'Set 7-day window' }));
    await waitFor(() => expect(mockUpdateField).toHaveBeenCalledWith('trash_retention_days', 7, 0));
  });

  it('keeps the select focusable through preview and confirmation, ignores duplicates, and restores focus on cancel', async () => {
    let preview!: (count: number) => void;
    mockInvoke.mockReturnValue(new Promise(resolve => { preview = resolve; }));
    vi.mocked(confirmDialog).mockImplementation(opts => confirmStore.confirm(opts));
    renderSection();
    render(ConfirmHost);
    const select = getTrashSelect();
    select.focus();
    await fireEvent.change(select, { target: { value: '7' } });
    expect(select.disabled).toBe(false);
    expect(document.activeElement).toBe(select);
    await fireEvent.change(select, { target: { value: '14' } });
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    expect(select.value).toBe('7');
    preview(1);
    await screen.findByRole('alertdialog');
    expect(screen.getByRole('alertdialog').textContent).toContain('1 recording already exceed the new limit');
    expect(select.disabled).toBe(false);
    const cancel = screen.getByRole('button', { name: 'Keep 30 days' });
    expect(document.activeElement).toBe(cancel);
    await fireEvent.change(select, { target: { value: '21' } });
    expect(confirmDialog).toHaveBeenCalledTimes(1);
    await fireEvent.click(cancel);
    await waitFor(() => expect(document.activeElement).toBe(select));
    expect(select.value).toBe('30');
    expect(mockUpdateField).not.toHaveBeenCalled();
    expect(screen.queryByText(/Seven days is the shortest recovery window/)).toBeNull();
  });

  it('restores the select value and focus after a confirmed save fails', async () => {
    mockUpdateField.mockRejectedValue(new Error('save failed'));
    vi.mocked(confirmDialog).mockImplementation(opts => confirmStore.confirm(opts));
    renderSection();
    render(ConfirmHost);
    const select = getTrashSelect();
    select.focus();
    await fireEvent.change(select, { target: { value: '14' } });
    await fireEvent.click(await screen.findByRole('button', { name: 'Set 14-day window' }));
    await waitFor(() => expect(select.value).toBe('30'));
    expect(document.activeElement).toBe(select);
  });
});
