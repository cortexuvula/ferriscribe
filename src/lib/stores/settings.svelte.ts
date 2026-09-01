import type { AppConfig } from '../types';
import { getSettings, saveSettings } from '../api/settings';

const defaults: AppConfig = {
  theme: 'dark',
  language: 'en-US',
  ai_provider: 'lmstudio',
  ai_model: '',
  ocr_model: null,
  whisper_model: 'large-v3-turbo',
  tts_provider: 'local',
  tts_voice: 'default',
  temperature: 0.2,
  input_device: null,
  sample_rate: 44100,
  autosave_enabled: true,
  autosave_interval_secs: 60,
  auto_generate_soap: false,
  soap_notification_sound: true,
  search_top_k: 5,
  mmr_lambda: 0.7,
  storage_path: null,
  lmstudio_host: 'localhost',
  lmstudio_port: 1234,
  lmstudio_disable_thinking: false,
  stt_mode: 'local',
  stt_remote_host: '',
  stt_remote_port: 8080,
  stt_remote_model: 'whisper-1',
  ollama_host: 'localhost',
  ollama_port: 11434,
  ollama_disable_thinking: false,
  omlx_host: 'localhost',
  omlx_port: 8000,
  omlx_disable_thinking: false,
  vocabulary_enabled: true,
  medical_dict_enabled: true,
  max_speakers: 3,
  custom_context_templates: [],
  custom_soap_prompt: null,
  custom_referral_prompt: null,
  custom_letter_prompt: null,
  custom_synopsis_prompt: null,
  custom_peer_discussion_prompt: null,
  icd_version: 'icd9',
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
  backup_target_url: null,
  backup_append_token: null,
  backup_dest_path: null,
  onboarding_completed: false,
  tos_accepted_at: null,
  auto_update_check: true,
  custom_conditions: [],
  sync_condition_chips: false,
  sync_content: false,
  retention_days: null,
};

class SettingsStore {
  state = $state<AppConfig>({ ...defaults });
  /** Reactive load flag. False until the backend returns the real config.
   *  Consumers gate on this to avoid flashing default-derived UI before the
   *  real values arrive (e.g. onboarding wizard before onboarding_completed
   *  is known to be true). */
  loaded = $state(false);
  /** Reactive error flag. Set when load() fails so App.svelte can render a
   *  retryable error UI instead of a permanent blank screen. Cleared on a
   *  successful reload. */
  loadError = $state(false);
  private saveQueue: Promise<void> = Promise.resolve();
  /** The last config known to be persisted server-side (from load(),
   *  successful saves, or the post-failure reload). updateField payloads
   *  are derived from THIS at drain time — never from the optimistic
   *  `state` snapshot — so a rolled-back change can never be resurrected
   *  by a later queued save that captured it. */
  private serverState: AppConfig | null = null;
  /**
   * Svelte-store-compatible subscribers. We use an explicit Set + manual
   * notification instead of `$effect.root` so `subscribe()` is safe to call
   * from any context (component effects OR plain module code like
   * endpointHealth's startPolling). The previous `$effect.root` shim leaked a
   * root scope per subscriber if the caller didn't invoke the teardown inside
   * a reactive context — this cannot leak, because notification is driven by
   * the explicit `notify()` calls on each state mutation below.
   */
  private subscribers = new Set<(value: AppConfig) => void>();

  /**
   * Svelte-store-compatible subscribe: emit the current value immediately,
   * then on every subsequent state change. Returns an unsubscribe function.
   */
  subscribe(cb: (value: AppConfig) => void): () => void {
    this.subscribers.add(cb);
    cb(this.state); // emit current value immediately (store contract)
    return () => { this.subscribers.delete(cb); };
  }

  /** Notify all subscribers of the current state. Called after every mutation
   *  that replaces `this.state`. */
  private notify(): void {
    for (const cb of this.subscribers) cb(this.state);
  }

  async load(): Promise<void> {
    try {
      const config = await getSettings();
      this.state = config;
      this.serverState = config;
      this.loaded = true;
      this.loadError = false;
      this.notify();
    } catch (err) {
      console.error('Failed to load settings:', err);
      // Mark the failure so the UI can surface a retryable error instead of
      // rendering a permanent blank screen. `loaded` stays false — the real
      // config never arrived, so gating on it still withholds the app shell.
      this.loadError = true;
    }
  }

  async save(config: AppConfig): Promise<void> {
    if (!this.loaded) {
      console.warn('Settings not loaded yet, refusing to save');
      return;
    }
    this.state = config;
    this.notify();
    const prev = this.saveQueue;
    this.saveQueue = (async () => {
      await prev.catch(() => {});
      try {
        await saveSettings(config);
        this.serverState = config;
      } catch (err) {
        console.error('Failed to save settings:', err);
        try {
          const latest = await getSettings();
          this.state = latest;
          this.serverState = latest;
          this.notify();
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
    this.notify();
    const prev = this.saveQueue;
    this.saveQueue = (async () => {
      await prev.catch(() => {});
      // Derive the payload at DRAIN time from the last known persisted
      // state + ONLY this call's delta. Sending the call-time optimistic
      // snapshot instead let a queued updateField resurrect a sibling
      // change whose save had just failed and been rolled back by the
      // reload above.
      const payload = { ...(this.serverState ?? this.state), [key]: value };
      try {
        await saveSettings(payload);
        this.serverState = payload;
      } catch (err) {
        console.error('Save failed:', err);
        try {
          const latest = await getSettings();
          this.state = latest;
          this.serverState = latest;
          this.notify();
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
