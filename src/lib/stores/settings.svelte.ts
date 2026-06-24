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
  input_device: null,
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
  medical_dict_enabled: true,
  max_speakers: 3,
  custom_context_templates: [],
  custom_soap_prompt: null,
  custom_referral_prompt: null,
  custom_letter_prompt: null,
  custom_synopsis_prompt: null,
  custom_peer_discussion_prompt: null,
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
  allow_public_endpoint: false,
  onboarding_completed: false,
  auto_update_check: true,
  custom_conditions: [],
};

class SettingsStore {
  state = $state<AppConfig>({ ...defaults });
  private loaded = false;
  private saveQueue: Promise<void> = Promise.resolve();

  /**
   * Svelte-store-compatible subscribe, backed by $effect.root so that
   * reactive consumers (e.g. endpointHealth) can track state changes.
   * Returns an unsubscribe function.
   */
  subscribe(cb: (value: AppConfig) => void): () => void {
    cb(this.state); // emit current value immediately (store contract)
    return $effect.root(() => {
      $effect(() => {
        cb(this.state);
      });
    });
  }

  async load(): Promise<void> {
    try {
      const config = await getSettings();
      this.state = config;
      this.loaded = true;
    } catch (err) {
      console.error('Failed to load settings:', err);
    }
  }

  async save(config: AppConfig): Promise<void> {
    if (!this.loaded) {
      console.warn('Settings not loaded yet, refusing to save');
      return;
    }
    this.state = config;
    const prev = this.saveQueue;
    this.saveQueue = (async () => {
      await prev.catch(() => {});
      try {
        await saveSettings(config);
      } catch (err) {
        console.error('Failed to save settings:', err);
        try {
          const latest = await getSettings();
          this.state = latest;
        } catch (_reloadErr) {
          // If reload also fails, leave local state as-is.
        }
        throw err;
      }
    })();
    return this.saveQueue;
  }

  async updateField<K extends keyof AppConfig>(
    key: K,
    value: AppConfig[K],
  ): Promise<void> {
    if (!this.loaded) {
      console.warn('Settings not loaded yet, refusing to save');
      return;
    }
    const next = { ...this.state, [key]: value };
    this.state = next;
    const prev = this.saveQueue;
    this.saveQueue = (async () => {
      await prev.catch(() => {});
      try {
        await saveSettings(next);
      } catch (err) {
        console.error('Save failed:', err);
        try {
          const latest = await getSettings();
          this.state = latest;
        } catch (_reloadErr) {
          // If reload also fails, leave local state as-is.
        }
        throw err;
      }
    })();
    return this.saveQueue;
  }
}

export const settings = new SettingsStore();
