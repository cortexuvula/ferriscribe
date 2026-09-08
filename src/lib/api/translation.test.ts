import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('./invokeWithOfflineHandling', () => ({
  // Route through the same mock so assertions cover both bridges; drop the
  // args key entirely when the wrapper passes none (invoke arg identity).
  invokeWithOfflineHandling: vi.fn(async (cmd: string, args?: unknown) =>
    args === undefined
      ? invokeMock(cmd)
      : invokeMock(cmd, args as Parameters<typeof invokeMock>[1])
  ),
  OfflineCancelled: class OfflineCancelled extends Error {},
}));

import { invoke } from '@tauri-apps/api/core';
import {
  captureStart,
  captureStop,
  clearSession,
  exportSession,
  getSession,
  speak,
  startSession,
  supportedLanguages,
  textUtterance,
} from './translation';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe('translation api', () => {
  it('session lifecycle wrappers invoke their commands with camelCase args', async () => {
    await supportedLanguages();
    expect(invokeMock).toHaveBeenCalledWith('translation_supported_languages');

    await startSession('es', 'en');
    expect(invokeMock).toHaveBeenCalledWith('translation_start_session', {
      patientLang: 'es',
      providerLang: 'en',
    });

    await getSession();
    expect(invokeMock).toHaveBeenCalledWith('translation_get_session');

    await clearSession();
    expect(invokeMock).toHaveBeenCalledWith('translation_clear_session');

    await exportSession();
    expect(invokeMock).toHaveBeenCalledWith('translation_export_session');
  });

  it('capture and text-utterance wrappers pass the speaker through', async () => {
    await captureStart('patient');
    expect(invokeMock).toHaveBeenCalledWith('translation_capture_start', {
      speaker: 'patient',
    });

    await captureStop();
    expect(invokeMock).toHaveBeenCalledWith('translation_capture_stop');

    await textUtterance('provider', 'hello');
    expect(invokeMock).toHaveBeenCalledWith('translation_text_utterance', {
      speaker: 'provider',
      text: 'hello',
    });
  });

  it('speak passes text and language', async () => {
    await speak('Buenos días', 'es');
    expect(invokeMock).toHaveBeenCalledWith('translation_speak', {
      text: 'Buenos días',
      language: 'es',
    });
  });
});
