import { writable } from 'svelte/store';
import type { AppConfig } from '../types';
import { getSettings, saveSettings } from '../api/settings';

const defaults: AppConfig = {
  theme: 'dark',
  language: 'en-US',
  ai_provider: 'lmstudio',
  ai_model: '',
  whisper_model: 'large-v3-turbo',
  tts_provider: 'elevenlabs',
  tts_voice: 'default',
  temperature: 0.2,
  sample_rate: 44100,
  autosave_enabled: true,
  autosave_interval_secs: 60,
  auto_generate_soap: false,
  search_top_k: 5,
  mmr_lambda: 0.7,
  storage_path: null,
  lmstudio_host: 'localhost',
  lmstudio_port: 1234,
  stt_mode: 'local',
  stt_remote_host: '',
  stt_remote_port: 8080,
  stt_remote_model: 'whisper-1',
  ollama_host: 'localhost',
  ollama_port: 11434,
  vocabulary_enabled: true,
  custom_context_templates: [],
  custom_soap_prompt: null,
  custom_referral_prompt: null,
  custom_letter_prompt: null,
  custom_synopsis_prompt: null,
  rsvp_wpm: 300,
  rsvp_font_size: 48,
  rsvp_chunk_size: 1,
  rsvp_dark_theme: true,
  rsvp_show_context: false,
  rsvp_audio_cue: false,
  rsvp_auto_start: true,
  rsvp_remember_sections: false,
  rsvp_remembered_sections: [],
  capture_for_training: false,
};

function createSettingsStore() {
  const { subscribe, set, update } = writable<AppConfig>(defaults);

  // Guard flag: prevents saving until a successful load has completed.
  // This protects Rust-only fields (soap_note_settings, agent_settings, etc.)
  // from being overwritten with incomplete TS defaults.
  let loaded = false;

  // Serialize all saves through a single promise chain. Without this, two
  // rapid updateField calls could race — if the first save fails and rolls
  // back after the second save has already flushed its (newer) state, the
  // local store ends up out of sync with the backend. Chaining guarantees
  // at most one inflight save, and a failed save re-reads the backend's
  // latest truth to re-sync.
  let saveQueue: Promise<void> = Promise.resolve();

  return {
    subscribe,

    async load(): Promise<void> {
      try {
        const config = await getSettings();
        set(config);
        loaded = true;
      } catch (err) {
        console.error('Failed to load settings:', err);
      }
    },

    async save(config: AppConfig): Promise<void> {
      if (!loaded) {
        console.warn('Settings not loaded yet, refusing to save');
        return;
      }
      set(config);
      const prev = saveQueue;
      saveQueue = (async () => {
        await prev.catch(() => {}); // tolerate prior failures
        try {
          await saveSettings(config);
        } catch (err) {
          console.error('Failed to save settings:', err);
          // Re-sync local state from backend so we don't diverge.
          try {
            const latest = await getSettings();
            set(latest);
          } catch (_reloadErr) {
            // If reload also fails, leave local state as-is.
          }
          throw err;
        }
      })();
      return saveQueue;
    },

    async updateField(key: string, value: any): Promise<void> {
      if (!loaded) {
        console.warn('Settings not loaded yet, refusing to save');
        return;
      }
      // Optimistic local update (synchronous).
      let next: AppConfig | null = null;
      update((current) => {
        next = { ...current, [key]: value };
        return next;
      });
      if (!next) return; // unreachable: update runs its callback synchronously
      const committed = next as AppConfig;

      // Serialize saves. Each save waits for the previous one before firing,
      // so failures and successes can't interleave across rapid updates.
      const prev = saveQueue;
      saveQueue = (async () => {
        await prev.catch(() => {}); // tolerate prior failures
        try {
          await saveSettings(committed);
        } catch (err) {
          console.error('Save failed:', err);
          // Reload backend's latest truth to keep local state consistent.
          try {
            const latest = await getSettings();
            set(latest);
          } catch (_reloadErr) {
            // If reload also fails, leave local state as-is.
          }
          throw err;
        }
      })();
      return saveQueue;
    },
  };
}

export const settings = createSettingsStore();
