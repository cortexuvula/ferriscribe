import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import {
  getSettings,
  saveSettings,
  testLmStudioConnection,
  testSttRemoteConnection,
  testOllamaConnection,
  setApiKey,
  getApiKey,
} from './settings';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe('settings api', () => {
  it('getSettings invokes get_settings with no args', async () => {
    await getSettings();
    expect(invokeMock).toHaveBeenCalledWith('get_settings');
  });

  it('saveSettings forwards the config under `config`', async () => {
    const config = { ai_provider: 'ollama' } as never;
    await saveSettings(config);
    expect(invokeMock).toHaveBeenCalledWith('save_settings', { config });
  });

  it('testLmStudioConnection passes host + port', async () => {
    await testLmStudioConnection('127.0.0.1', 1234);
    expect(invokeMock).toHaveBeenCalledWith('test_lmstudio_connection', { host: '127.0.0.1', port: 1234 });
  });

  it('testSttRemoteConnection passes host + port + apiKey (preserves null)', async () => {
    await testSttRemoteConnection('h', 8080, null);
    expect(invokeMock).toHaveBeenCalledWith('test_stt_remote_connection', { host: 'h', port: 8080, apiKey: null });
    invokeMock.mockReset();
    await testSttRemoteConnection('h', 8080, 'secret');
    expect(invokeMock).toHaveBeenCalledWith('test_stt_remote_connection', { host: 'h', port: 8080, apiKey: 'secret' });
  });

  it('testOllamaConnection passes host + port', async () => {
    await testOllamaConnection('127.0.0.1', 11434);
    expect(invokeMock).toHaveBeenCalledWith('test_ollama_connection', { host: '127.0.0.1', port: 11434 });
  });

  it('setApiKey / getApiKey pass provider (and key on set)', async () => {
    await setApiKey('openai', 'sk-X');
    expect(invokeMock).toHaveBeenLastCalledWith('set_api_key', { provider: 'openai', key: 'sk-X' });
    await getApiKey('openai');
    expect(invokeMock).toHaveBeenLastCalledWith('get_api_key', { provider: 'openai' });
  });
});
