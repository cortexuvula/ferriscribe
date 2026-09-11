// @vitest-environment jsdom
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/svelte';
import { tick } from 'svelte';
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import SettingsContent from './SettingsContent.svelte';
import SettingsDialog from '../dialogs/SettingsDialog.svelte';
import { settingsNav, type SettingsSection } from '../stores/settingsNav.svelte';
import { settings } from '../stores/settings.svelte';
import { confirmDialog } from '../stores/confirm.svelte';

// Keep the actual Prompts editor/dirty guard and compositions; mock only
// unrelated heavyweight panes and all backend I/O. No app/data startup.
vi.mock('./settings/Models.svelte', () => import('./settings/test-fixtures/ModelsPane.svelte'));
vi.mock('./settings/Audio.svelte', () => import('./settings/test-fixtures/AudioPane.svelte'));
vi.mock('./settings/Backup.svelte', () => import('./settings/test-fixtures/BackupPane.svelte'));
vi.mock('./settings/Sharing.svelte', () => import('./settings/test-fixtures/SharingPane.svelte'));
vi.mock('./settings/TrainingCorpus.svelte', () => import('./settings/test-fixtures/TrainingCorpusPane.svelte'));
vi.mock('./settings/LetterAudiences.svelte', () => import('./settings/test-fixtures/LetterAudiencesPane.svelte'));
vi.mock('./settings/About.svelte', () => import('./settings/test-fixtures/AboutPane.svelte'));
vi.mock('../api/prompts', () => ({ getDefaultPrompt: vi.fn(async () => 'Synthetic prompt') }));
vi.mock('../api/specialty', () => ({ listSpecialtyPacks: vi.fn(async () => []), getSpecialtyPackPrompt: vi.fn(async () => null) }));
vi.mock('../stores/confirm.svelte', () => ({ confirmDialog: vi.fn(async () => true) }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async (cmd: string) => {
  if (cmd === 'get_vocabulary_count') return [0, 0];
  if (cmd === 'database_encryption_status') return { state: 'encrypted', key_present: true };
  return [];
}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(), save: vi.fn() }));

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(confirmDialog).mockResolvedValue(true);
  settingsNav.clear();
  settingsNav.state.lastSection = 'general';
  settings.state.allow_public_endpoint = false;
  settings.state.specialty = null;
  settings.state.custom_soap_prompt = null;
});
afterEach(cleanup);

async function dirtyPrompts(dialog = false) {
  settingsNav.state.lastSection = 'prompts';
  const view = dialog ? render(SettingsDialog, { open: true }) : render(SettingsContent);
  await waitFor(() => expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('Synthetic prompt'));
  await fireEvent.input(screen.getByRole('textbox'), { target: { value: 'Synthetic edited draft' } });
  return view;
}
function deferred() {
  let resolve!: (value: boolean) => void;
  const promise = new Promise<boolean>((r) => { resolve = r; });
  return { promise, resolve };
}
async function request(section: SettingsSection) {
  settingsNav.navigateTo(section);
  await tick();
}

describe('settings navigation and composition', () => {
  it('opts only the Settings dialog into a fixed-height single-scroll shell', () => {
    render(SettingsDialog, { open: true });
    expect(document.querySelector('.modal-container.settings-shell')).toBeTruthy();
  });

  it('groups the focused destinations under labelled navigation', () => {
    render(SettingsContent);
    expect(screen.getByRole('navigation', { name: 'Settings sections' })).toBeTruthy();
    for (const name of ['Everyday', 'Clinical content', 'System']) expect(screen.getByRole('heading', { name })).toBeTruthy();
    for (const name of ['General', 'Recording & Transcription', 'AI Models', 'Prompts & Specialties', 'Vocabulary & Templates', 'Letter Audiences', 'Storage & Backup', 'Sharing', 'Advanced', 'About & Updates']) {
      expect(screen.getByRole('button', { name })).toBeTruthy();
    }
  });

  it.each(['general', 'prompts', 'models', 'audio', 'backup', 'sharing', 'training-corpus', 'letter-audiences', 'about', 'vocabulary', 'advanced'] as SettingsSection[])('retains external route %s and renders its pane', async (section) => {
    render(SettingsContent);
    await request(section);
    await waitFor(() => expect(settingsNav.state.lastSection).toBe(section));
    expect(settingsNav.state.requestedSection).toBeNull();
    const markers: Record<SettingsSection, string> = {
      general: 'General', prompts: 'Prompts & Specialties', models: 'Synthetic Models destination',
      audio: 'Synthetic Audio destination', backup: 'Synthetic Backup destination',
      sharing: 'Synthetic Sharing destination', 'training-corpus': 'Synthetic TrainingCorpus destination',
      'letter-audiences': 'Synthetic LetterAudiences destination', about: 'Synthetic About destination',
      vocabulary: 'Vocabulary & Templates', advanced: 'Advanced',
    };
    expect(screen.getByRole('heading', { name: markers[section] })).toBeTruthy();
  });

  it('keeps General limited to everyday preferences', () => {
    render(SettingsContent);
    expect(screen.getByLabelText('Theme')).toBeTruthy();
    expect(screen.getByLabelText('Enable Autosave')).toBeTruthy();
    expect(screen.getByLabelText('Autosave Interval (seconds)')).toBeTruthy();
    expect(screen.getByLabelText('Play a sound when a SOAP note is generated')).toBeTruthy();
    for (const text of ['Transcription Language', 'Recording Storage Folder', 'Database Security', 'Recording Retention', 'Custom Vocabulary', 'Setup Wizard', 'Screenshot Region OCR']) expect(screen.queryByText(text)).toBeNull();
  });

  it('places storage, security and retention alongside backup', async () => {
    settingsNav.state.lastSection = 'backup';
    render(SettingsContent);
    expect(screen.getByText('Recording Storage Folder')).toBeTruthy();
    expect(screen.getByText('Database Security')).toBeTruthy();
    expect(screen.getByText('Recording Retention')).toBeTruthy();
    expect(screen.getByTestId('synthetic-pane')).toBeTruthy();
  });

  it('places vocabulary, templates and dictionary in one destination without retention', async () => {
    settingsNav.state.lastSection = 'vocabulary';
    render(SettingsContent);
    for (const name of ['Manage Vocabulary', 'Manage Templates', 'Manage Dictionary']) expect(screen.getByRole('button', { name })).toBeTruthy();
    expect(screen.queryByText('Recording Retention')).toBeNull();
  });

  it('exposes setup, OCR, endpoint exception, training capture and legacy corpus from Advanced', async () => {
    settingsNav.state.lastSection = 'advanced';
    render(SettingsContent);
    expect(screen.getByRole('button', { name: 'Re-run setup' })).toBeTruthy();
    expect(screen.getByText('Screenshot Region OCR')).toBeTruthy();
    expect(screen.getByRole('checkbox', { name: /Allow public AI/ })).toBeTruthy();
    expect(screen.getByRole('checkbox', { name: /Capture generations/ })).toBeTruthy();
    await fireEvent.click(screen.getByRole('button', { name: 'Open Training Corpus' }));
    await waitFor(() => expect(settingsNav.state.lastSection).toBe('training-corpus'));
  });

  it('keeps the public-endpoint warning visible outside Advanced on every destination', () => {
    settings.state.allow_public_endpoint = true;
    settingsNav.state.lastSection = 'models';
    render(SettingsContent);
    expect(screen.getByText('Public endpoints enabled.')).toBeTruthy();
  });

  it('reopens at the last successfully visited destination', async () => {
    const view = render(SettingsContent);
    await fireEvent.click(screen.getByRole('button', { name: 'AI Models' }));
    await waitFor(() => expect(settingsNav.state.lastSection).toBe('models'));
    view.unmount();
    render(SettingsContent);
    expect(screen.getByRole('button', { name: 'AI Models' }).getAttribute('aria-current')).toBe('true');
  });
});

describe('all settings exits share the real Prompts discard guard', () => {
  it.each([false, true])('guards sidebar switching (discard=%s)', async (discard) => {
    await dirtyPrompts();
    vi.mocked(confirmDialog).mockResolvedValue(discard);
    await fireEvent.click(screen.getByRole('button', { name: 'General' }));
    await waitFor(() => expect(confirmDialog).toHaveBeenCalledTimes(1));
    expect(settingsNav.state.lastSection).toBe(discard ? 'general' : 'prompts');
    if (!discard) expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('Synthetic edited draft');
  });

  it.each([false, true])('guards external requests (discard=%s)', async (discard) => {
    await dirtyPrompts();
    vi.mocked(confirmDialog).mockResolvedValue(discard);
    await request('models');
    await waitFor(() => expect(confirmDialog).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(settingsNav.state.requestedSection).toBeNull());
    expect(settingsNav.state.lastSection).toBe(discard ? 'models' : 'prompts');
  });

  it('same-section external requests are no-ops, retaining the unsaved draft', async () => {
    await dirtyPrompts();
    await request('prompts');
    expect(confirmDialog).not.toHaveBeenCalled();
    expect(settingsNav.state.requestedSection).toBeNull();
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('Synthetic edited draft');
  });

  it('same-section sidebar clicks never ask to discard', async () => {
    await dirtyPrompts();
    await fireEvent.click(screen.getByRole('button', { name: /Prompts/ }));
    expect(confirmDialog).not.toHaveBeenCalled();
  });

  it('uses one pending confirmation and the newest external destination', async () => {
    await dirtyPrompts();
    const held = deferred();
    vi.mocked(confirmDialog).mockReturnValue(held.promise);
    await request('models');
    await request('audio');
    expect(settingsNav.state.requestedSection).toBe('audio');
    expect(confirmDialog).toHaveBeenCalledTimes(1);
    held.resolve(true);
    await waitFor(() => expect(settingsNav.state.lastSection).toBe('audio'));
    expect(settingsNav.state.requestedSection).toBeNull();
  });

  it('declining a pending confirmation leaves the draft and consumes the latest request', async () => {
    await dirtyPrompts();
    const held = deferred();
    vi.mocked(confirmDialog).mockReturnValue(held.promise);
    await request('models');
    await request('audio');
    held.resolve(false);
    await waitFor(() => expect(settingsNav.state.requestedSection).toBeNull());
    expect(settingsNav.state.lastSection).toBe('prompts');
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('Synthetic edited draft');
  });

  it.each(['button', 'escape', 'backdrop'])('guards %s close and allows a later confirmed close', async (path) => {
    await dirtyPrompts(true);
    vi.mocked(confirmDialog).mockResolvedValue(false);
    const close = async () => {
      if (path === 'button') await fireEvent.click(screen.getByRole('button', { name: 'Close dialog' }));
      else if (path === 'escape') await fireEvent.keyDown(window, { key: 'Escape' });
      else await fireEvent.click(screen.getByRole('dialog', { name: 'Settings' }));
    };
    await close();
    await waitFor(() => expect(confirmDialog).toHaveBeenCalledTimes(1));
    expect(screen.getByRole('dialog', { name: 'Settings' })).toBeTruthy();
    vi.mocked(confirmDialog).mockResolvedValue(true);
    await close();
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Settings' })).toBeNull());
  });

  it('a newer navigation supersedes a pending close without closing the dialog', async () => {
    await dirtyPrompts(true);
    const held = deferred();
    vi.mocked(confirmDialog).mockReturnValue(held.promise);
    await fireEvent.click(screen.getByRole('button', { name: 'Close dialog' }));
    await request('models');
    held.resolve(true);
    await waitFor(() => expect(settingsNav.state.lastSection).toBe('models'));
    expect(screen.getByRole('dialog', { name: 'Settings' })).toBeTruthy();
    expect(confirmDialog).toHaveBeenCalledTimes(1);
  });
});
