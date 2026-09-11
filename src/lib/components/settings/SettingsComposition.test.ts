// @vitest-environment jsdom
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import General from './General.svelte';
import RecordingStorage from './sections/RecordingStorage.svelte';
import SetupWizardSection from './sections/SetupWizardSection.svelte';
import AdvancedSettings from './sections/AdvancedSettings.svelte';
import VocabularySettings from './VocabularySettings.svelte';
import { settings } from '../../stores/settings.svelte';
import { confirmDialog } from '../../stores/confirm.svelte';
import { open } from '@tauri-apps/plugin-dialog';
import { invoke } from '@tauri-apps/api/core';
import { reinitProviders } from '../../api/chat';
import { playSoapCompleteChime } from '../../utils/notificationSound';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(), save: vi.fn() }));
vi.mock('../../api/chat', () => ({ reinitProviders: vi.fn(async () => {}) }));
vi.mock('../../utils/notificationSound', () => ({ playSoapCompleteChime: vi.fn() }));
vi.mock('../../stores/confirm.svelte', () => ({ confirmDialog: vi.fn(async () => true) }));

beforeEach(() => {
  vi.clearAllMocks();
  vi.spyOn(settings, 'updateField').mockResolvedValue(undefined);
  vi.mocked(open).mockResolvedValue(null);
  vi.mocked(confirmDialog).mockResolvedValue(true);
  vi.mocked(invoke).mockImplementation(async (cmd) => cmd === 'get_vocabulary_count' ? [0, 0] : []);
  settings.state.theme = 'dark';
  settings.state.autosave_enabled = true;
  settings.state.autosave_interval_secs = 60;
  settings.state.soap_notification_sound = false;
  settings.state.storage_path = null;
  settings.state.allow_public_endpoint = false;
  settings.state.capture_for_training = false;
});
afterEach(() => { cleanup(); vi.restoreAllMocks(); });

describe('moved settings retain persistence and confirmation contracts', () => {
  it('persists theme, autosave and sound; previews sound only when enabling', async () => {
    render(General);
    await fireEvent.change(screen.getByLabelText('Theme'), { target: { value: 'light' } });
    expect(settings.updateField).toHaveBeenCalledWith('theme', 'light');
    await fireEvent.click(screen.getByLabelText('Enable Autosave'));
    expect(settings.updateField).toHaveBeenCalledWith('autosave_enabled', false);
    await fireEvent.click(screen.getByLabelText('Play a sound when a SOAP note is generated'));
    expect(settings.updateField).toHaveBeenCalledWith('soap_notification_sound', true);
    expect(playSoapCompleteChime).toHaveBeenCalledTimes(1);
  });

  it('rejects invalid autosave intervals without persisting and restores the saved value', async () => {
    render(General);
    const input = screen.getByLabelText('Autosave Interval (seconds)') as HTMLInputElement;
    await fireEvent.change(input, { target: { value: '9' } });
    expect(screen.getByRole('alert').textContent).toContain('10 and 600');
    expect(input.value).toBe('60');
    expect(settings.updateField).not.toHaveBeenCalled();
    await fireEvent.change(input, { target: { value: '120' } });
    expect(settings.updateField).toHaveBeenCalledWith('autosave_interval_secs', 120);
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('disables autosave interval when autosave is off', () => {
    settings.state.autosave_enabled = false;
    render(General);
    expect((screen.getByLabelText('Autosave Interval (seconds)') as HTMLInputElement).disabled).toBe(true);
  });

  it('keeps storage selection directory-only and cancellation does not save', async () => {
    render(RecordingStorage);
    await fireEvent.click(screen.getByRole('button', { name: 'Browse' }));
    expect(open).toHaveBeenCalledWith({ directory: true, multiple: false, title: 'Select Recording Storage Folder' });
    expect(settings.updateField).not.toHaveBeenCalled();
    vi.mocked(open).mockResolvedValue('/synthetic/recordings');
    await fireEvent.click(screen.getByRole('button', { name: 'Browse' }));
    expect(settings.updateField).toHaveBeenCalledWith('storage_path', '/synthetic/recordings');
  });

  it('resets a custom storage directory to null', async () => {
    settings.state.storage_path = '/synthetic/recordings';
    render(RecordingStorage);
    await fireEvent.click(screen.getByRole('button', { name: 'Reset' }));
    expect(settings.updateField).toHaveBeenCalledWith('storage_path', null);
  });

  it('reruns onboarding through the same persisted flag', async () => {
    render(SetupWizardSection);
    await fireEvent.click(screen.getByRole('button', { name: 'Re-run setup' }));
    expect(settings.updateField).toHaveBeenCalledWith('onboarding_completed', false);
  });

  it('declining the public endpoint exception restores the checkbox and never reinitializes providers', async () => {
    vi.mocked(confirmDialog).mockResolvedValue(false);
    render(AdvancedSettings);
    const checkbox = screen.getByRole('checkbox', { name: /Allow public AI/ }) as HTMLInputElement;
    await fireEvent.click(checkbox);
    await waitFor(() => expect(checkbox.checked).toBe(false));
    expect(settings.updateField).not.toHaveBeenCalled();
    expect(reinitProviders).not.toHaveBeenCalled();
  });

  it('accepting the public endpoint exception persists before provider reinitialization', async () => {
    render(AdvancedSettings);
    await fireEvent.click(screen.getByRole('checkbox', { name: /Allow public AI/ }));
    await waitFor(() => expect(reinitProviders).toHaveBeenCalledOnce());
    expect(settings.updateField).toHaveBeenCalledWith('allow_public_endpoint', true);
    expect(confirmDialog).toHaveBeenCalledWith(expect.objectContaining({ danger: true, confirmLabel: 'Allow' }));
    expect(vi.mocked(settings.updateField).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(reinitProviders).mock.invocationCallOrder[0]);
  });

  it('training capture remains a local settings flag', async () => {
    render(AdvancedSettings);
    await fireEvent.click(screen.getByRole('checkbox', { name: /Capture generations/ }));
    expect(settings.updateField).toHaveBeenCalledWith('capture_for_training', true);
  });

  it('vocabulary and dictionary failures stay unknown, never displayed as zero', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    vi.mocked(invoke).mockRejectedValue(new Error('Synthetic unavailable'));
    render(VocabularySettings);
    await waitFor(() => expect(screen.getAllByText('Count unavailable')).toHaveLength(2));
    expect(screen.queryByText('0 entries (0 enabled)')).toBeNull();
    expect(screen.queryByText('0 words saved')).toBeNull();
  });
});
