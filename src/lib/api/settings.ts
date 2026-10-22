import { invoke } from '@tauri-apps/api/core';
import type { AppConfig } from '../types';

export async function getSettings(): Promise<AppConfig> {
  return invoke('get_settings');
}

/** A shorter Trash window requires the caller's freshly confirmed purge count. */
export async function saveSettings(
  config: AppConfig,
  confirmedTrashPurgeCount?: number,
): Promise<void> {
  return invoke('save_settings', {
    config,
    ...(confirmedTrashPurgeCount === undefined ? {} : { confirmedTrashPurgeCount }),
  });
}

/// Mark onboarding as started. Called by the wizard the first time it saves any
/// config, so an interrupted wizard reappears on next launch instead of being
/// silently auto-marked complete. Idempotent.
export async function setOnboardingStarted(): Promise<void> {
  return invoke('set_onboarding_started');
}

export async function testLmStudioConnection(
  host: string,
  port: number,
  apiKey?: string | null,
): Promise<string> {
  return invoke('test_lmstudio_connection', { host, port, apiKey: apiKey ?? null });
}

export async function testSttRemoteConnection(
  host: string,
  port: number,
  apiKey: string | null,
): Promise<string> {
  return invoke('test_stt_remote_connection', { host, port, apiKey });
}

export async function testOllamaConnection(
  host: string,
  port: number,
  apiKey?: string | null,
): Promise<string> {
  return invoke('test_ollama_connection', { host, port, apiKey: apiKey ?? null });
}

export async function testOmlxConnection(
  host: string,
  port: number,
  apiKey?: string | null,
): Promise<string> {
  return invoke('test_omlx_connection', { host, port, apiKey: apiKey ?? null });
}

export async function setApiKey(provider: string, key: string): Promise<void> {
  return invoke('set_api_key', { provider, key });
}

export async function getApiKey(provider: string): Promise<string | null> {
  return invoke('get_api_key', { provider });
}
