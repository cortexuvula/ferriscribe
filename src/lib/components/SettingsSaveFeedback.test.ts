// @vitest-environment jsdom
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/svelte';
import { beforeEach, afterEach, expect, it, vi } from 'vitest';
import SettingsContent from './SettingsContent.svelte';
import { settings } from '../stores/settings.svelte';
import { settingsNav } from '../stores/settingsNav.svelte';
import { getSettings, saveSettings } from '../api/settings';
import { theme } from '../stores/theme.svelte';

vi.mock('../api/settings', async (original) => ({
  ...await original<typeof import('../api/settings')>(),
  getSettings: vi.fn(), saveSettings: vi.fn(), getApiKey: vi.fn(async () => null),
}));
vi.mock('../api/chat', () => ({ listModels: vi.fn(async () => [{ id: 'synthetic', name: 'Synthetic', provider: 'lmstudio' }]), setActiveProvider: vi.fn(), reinitProviders: vi.fn() }));
vi.mock('../api/prompts', () => ({ getDefaultPrompt: vi.fn(async () => 'Synthetic prompt') }));
vi.mock('../api/specialty', () => ({ listSpecialtyPacks: vi.fn(async () => []), getSpecialtyPackPrompt: vi.fn(async () => null) }));
vi.mock('../api/audio', () => ({ listAudioDevices: vi.fn(async () => []), runMicrophoneProbe: vi.fn() }));
vi.mock('../api/models', () => ({ listWhisperModels: vi.fn(async () => []), listPyannoteModels: vi.fn(async () => []), downloadModel: vi.fn(), deleteModel: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(), save: vi.fn() }));
const initial = JSON.parse(JSON.stringify(settings.state));
beforeEach(async () => {
  vi.clearAllMocks();
  vi.mocked(getSettings).mockResolvedValue({ ...initial, ai_model: 'synthetic' });
  vi.mocked(saveSettings).mockResolvedValue(undefined);
  await settings.load();
  settings.saveError = null;
  settingsNav.clear();
  settingsNav.state.lastSection = 'general';
});
afterEach(cleanup);
function failSave() { vi.mocked(saveSettings).mockRejectedValueOnce(new Error('Synthetic disk failure')); }

it('renders an actual failed save and restores the General control and theme', async () => {
  render(SettingsContent);
  failSave();
  await fireEvent.change(screen.getByLabelText('Theme'), { target: { value: 'light' } });
  await waitFor(() => expect(screen.getByRole('alert').textContent).toContain('Could not save settings'));
  expect((screen.getByLabelText('Theme') as HTMLSelectElement).value).toBe('dark');
  expect(theme.current).toBe('dark');
  expect(getSettings).toHaveBeenCalledTimes(2);
});
it('preserves rejection semantics and reports an unsuccessful reload truthfully', async () => {
  failSave();
  vi.mocked(getSettings).mockRejectedValueOnce(new Error('Synthetic reload failure'));
  await expect(settings.updateField('temperature', 0.8)).rejects.toThrow('Synthetic disk failure');
  expect(settings.saveError).toContain('could not reload');
  expect(settings.saveError).not.toContain('restored');
  expect(settings.state.temperature).toBe(0.8);
  await settings.updateField('temperature', 0.4);
  expect(settings.saveError).toBeNull();
});
it.each([
  ['models', /Temperature/, '0.8', '0.2'],
  ['audio', /Max speakers/, '6', '3'],
] as const)('resynchronizes the %s slider after a failed API save', async (section, label, edited, restored) => {
  settingsNav.state.lastSection = section;
  render(SettingsContent);
  const slider = screen.getByLabelText(label) as HTMLInputElement;
  await fireEvent.input(slider, { target: { value: edited } });
  expect(saveSettings).not.toHaveBeenCalled();
  failSave();
  await fireEvent.change(slider);
  await waitFor(() => expect(slider.value).toBe(restored));
  expect(settings.saveError).toContain('Could not save settings');
});
it('restores the transcription mode radio and engine after failure', async () => {
  settingsNav.state.lastSection = 'audio';
  render(SettingsContent);
  failSave();
  await fireEvent.click(screen.getByLabelText('Remote'));
  await waitFor(() => expect(settings.saveError).toBeTruthy());
  expect((screen.getByLabelText('Local') as HTMLInputElement).checked).toBe(true);
  expect(screen.getByLabelText('Whisper Model')).toBeTruthy();
});
it('retains an unsaved prompt draft when explicit save fails', async () => {
  settingsNav.state.lastSection = 'prompts';
  render(SettingsContent);
  await waitFor(() => expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('Synthetic prompt'));
  await fireEvent.input(screen.getByRole('textbox'), { target: { value: 'Synthetic edited draft' } });
  failSave();
  await fireEvent.click(screen.getByRole('button', { name: /Save/ }));
  await waitFor(() => expect(settings.saveError).toBeTruthy());
  expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('Synthetic edited draft');
});
it('explains immediate preferences versus explicitly saved prompts', async () => {
  render(SettingsContent);
  expect(screen.getByText(/Changes apply immediately/)).toBeTruthy();
  await fireEvent.click(screen.getByRole('button', { name: 'AI Models' }));
  expect(screen.getByText(/Changes apply immediately/)).toBeTruthy();
  await fireEvent.click(screen.getByRole('button', { name: 'Prompts & Specialties' }));
  expect(screen.getByText(/Prompt edits are saved only/)).toBeTruthy();
});
