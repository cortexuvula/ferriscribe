// @vitest-environment jsdom
import { render, screen, fireEvent, waitFor, cleanup, within } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import Audio from './Audio.svelte';

const state = vi.hoisted(() => ({
  language: 'en-US', input_device: null, stt_mode: 'local', whisper_model: 'tiny',
  max_speakers: 3, sample_rate: 44100, auto_generate_soap: false,
  stt_remote_host: 'localhost', stt_remote_port: 8080, stt_remote_model: 'whisper-1',
}));
const updateField = vi.hoisted(() => vi.fn(async () => {}));
const download = vi.hoisted(() => vi.fn(async () => {}));
const reinit = vi.hoisted(() => vi.fn(async () => {}));
const unlisten = vi.hoisted(() => vi.fn());
const listPyannote = vi.hoisted(() => vi.fn(async () => [
  { id: 'speaker-model', description: 'Synthetic speaker model', size_bytes: 1024, downloaded: false },
]));
vi.mock('../../stores/settings.svelte', () => ({ settings: { state, updateField } }));
vi.mock('../../api/audio', () => ({
  listAudioDevices: vi.fn(async () => [{ name: 'Synthetic microphone', is_default: true }]),
  runMicrophoneProbe: vi.fn(),
}));
vi.mock('../../api/models', () => ({
  listWhisperModels: vi.fn(async () => [{ id: 'tiny', description: 'Synthetic engine', size_bytes: 1024, downloaded: true }]),
  listPyannoteModels: listPyannote, downloadModel: download, deleteModel: vi.fn(),
}));
vi.mock('../../api/chat', () => ({ reinitProviders: reinit }));
vi.mock('../../api/settings', () => ({
  getApiKey: vi.fn(async () => null), setApiKey: vi.fn(), testSttRemoteConnection: vi.fn(),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => unlisten) }));
vi.mock('../../stores/toasts.svelte', () => ({ toasts: { error: vi.fn(), success: vi.fn() } }));
vi.mock('../../stores/confirm.svelte', () => ({ confirmDialog: vi.fn(async () => true) }));

describe('Audio focused layout', () => {
  beforeEach(() => { vi.clearAllMocks(); });
  afterEach(cleanup);

  it('keeps microphone, language, mode and engine above advanced settings', async () => {
    render(Audio);
    await waitFor(() => expect((screen.getByLabelText('Whisper Model') as HTMLSelectElement).options.length).toBe(1));
    const advanced = screen.getByText('Advanced transcription').closest('details')!;
    expect(advanced.open).toBe(false);
    for (const label of ['Input Device', 'Transcription Language', 'Whisper Model']) {
      expect(screen.getByLabelText(label).closest('details')).toBeNull();
    }
    expect(screen.getByText('Local').closest('details')).toBeNull();
    expect(advanced.querySelector('summary')?.textContent).toContain('3 speakers');
    expect(advanced.querySelector('summary')?.textContent).toContain('44100 Hz');
    expect(advanced.querySelector('summary')?.textContent).toContain('Speaker models needed');
    expect(screen.getByLabelText('Sample Rate').closest('details')).toBe(advanced);
    expect(screen.getByLabelText(/Max speakers/).closest('details')).toBe(advanced);
    await fireEvent.change(screen.getByLabelText('Transcription Language'), { target: { value: 'fr-FR' } });
    expect(updateField).toHaveBeenCalledWith('language', 'fr-FR');
    advanced.open = true;
    await fireEvent.change(screen.getByLabelText('Sample Rate'), { target: { value: '16000' } });
    expect(updateField).toHaveBeenCalledWith('sample_rate', 16000);
    updateField.mockClear();
    await fireEvent.input(screen.getByLabelText(/Max speakers/), { target: { value: '4' } });
    expect(updateField).not.toHaveBeenCalled();
    await fireEvent.change(screen.getByLabelText(/Max speakers/));
    expect(updateField).toHaveBeenCalledWith('max_speakers', 4);
  });

  it('keeps download progress in the collapsed advanced summary and failures outside it', async () => {
    let rejectDownload!: (reason: Error) => void;
    download.mockImplementationOnce(() => new Promise((_resolve, reject) => { rejectDownload = reject; }));
    render(Audio);
    await screen.findByText('Synthetic speaker model');
    const advanced = screen.getByText('Advanced transcription').closest('details')!;
    advanced.open = true;
    await fireEvent.click(within(advanced).getByRole('button', { name: 'Download' }));
    advanced.open = false;
    await waitFor(() => expect(advanced.querySelector('summary')?.textContent).toContain('Downloading speaker-model'));
    rejectDownload(new Error('Synthetic download unavailable'));
    const error = await screen.findByRole('alert');
    expect(error.textContent).toContain('Synthetic download unavailable');
    expect(error.closest('details')).toBeNull();
  });

  it('shows speaker model list failures outside advanced settings', async () => {
    listPyannote.mockRejectedValueOnce(new Error('Synthetic model list unavailable'));
    render(Audio);
    const error = await screen.findByRole('alert');
    expect(error.textContent).toContain('Synthetic model list unavailable');
    expect(error.closest('details')).toBeNull();
  });

  it('retains remote engine controls and reinitializes on mode change', async () => {
    render(Audio);
    await screen.findByText('Synthetic speaker model');
    await fireEvent.click(screen.getByLabelText('Remote'));
    await waitFor(() => expect(reinit).toHaveBeenCalled());
    expect(updateField).toHaveBeenCalledWith('stt_mode', 'remote');
    expect(screen.getByLabelText('Host').closest('details')).toBeNull();
    expect(screen.getByLabelText('Port')).toBeTruthy();
    expect(screen.getByLabelText('API key (optional)')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Test Connection' })).toBeTruthy();
  });
});
