import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import * as translationApi from '../api/translation';
import type {
  TranslationEntry,
  TranslationLanguage,
  TranslationSpeaker,
} from '../api/translation';
import { formatError } from '../types/errors';
import { OfflineCancelled } from '../api/invokeWithOfflineHandling';

export type TranslationPhase = 'idle' | 'recording' | 'transcribing' | 'translating';

/** Hard cap on a single tap-to-talk capture, in seconds. Guards against a
 *  forgotten tap writing an ever-growing utterance WAV. */
export const MAX_UTTERANCE_SECS = 120;

/**
 * Singleton store for the Translate tab's conversation state.
 *
 * The backend session (`commands/translation.rs`) is the source of truth;
 * this store mirrors it for the UI and drives the tap-to-talk lifecycle:
 * `capture(speaker)` toggles a capture (idle → recording → transcribing →
 * translating → idle), `submitText` is the typed fallback. The store is a
 * module singleton, so an in-flight utterance keeps completing even if the
 * user switches tabs mid-processing.
 */
class TranslationStore {
  entries = $state<TranslationEntry[]>([]);
  phase = $state<TranslationPhase>('idle');
  activeSpeaker = $state<TranslationSpeaker | null>(null);

  /** Language pair for the session (provider = source, patient = target). */
  providerLang = $state('');
  patientLang = $state('');

  /** Supported-language list, fetched once on init. */
  languages = $state<TranslationLanguage[]>([]);

  /** Live level meter for the in-flight capture (~128 peaks per 50 ms). */
  waveform = $state<number[]>([]);
  /** Seconds since the current capture started. */
  elapsed = $state(0);
  error = $state<string | null>(null);
  /** Soft, auto-dismissing status message for EXPECTED outcomes (mistimed
   *  tap, silence, nothing heard) — unlike `error`, it never blocks. */
  notice = $state<string | null>(null);

  private waveformUnlisten: UnlistenFn | null = null;
  private progressUnlisten: UnlistenFn | null = null;
  private timer: ReturnType<typeof setInterval> | null = null;
  private noticeTimer: ReturnType<typeof setTimeout> | null = null;
  private _busy = false;
  /** Whether the backend session exists (it dies with the app process).
   *  Non-reactive — only action paths consult it, via ensureSession(). */
  private sessionActive = false;

  get busy(): boolean {
    return this._busy;
  }

  /** Show a soft notice that dismisses itself after a few seconds. */
  setNotice(message: string, durationMs = 4000) {
    if (this.noticeTimer) clearTimeout(this.noticeTimer);
    this.notice = message;
    this.noticeTimer = setTimeout(() => {
      this.notice = null;
      this.noticeTimer = null;
    }, durationMs);
  }

  dismissNotice() {
    if (this.noticeTimer) clearTimeout(this.noticeTimer);
    this.noticeTimer = null;
    this.notice = null;
  }

  /** One-time (per app run) setup: language list + progress events +
   *  session rehydration (e.g. after a webview reload). */
  async init() {
    if (this.progressUnlisten) return;
    try {
    this.progressUnlisten = await listen<string>('translation-progress', (event) => {
      const stage = event.payload;
      if (stage === 'transcribing' || stage === 'translating') {
        this.phase = stage;
      }
      // "complete" needs no handling: the awaiting capture()/stop path
      // appends the entry and flips the phase to idle itself.
    });
    } catch {
      // Events are cosmetic (phase labels); failure is non-fatal.
    }
    try {
      this.languages = await translationApi.supportedLanguages();
    } catch (e) {
      this.error = formatError(e) || 'Could not load supported languages';
    }
    await this.rehydrate();
  }

  /** Pull the backend session back into view (entries + language pair). */
  async rehydrate() {
    try {
      const session = await translationApi.getSession();
      this.sessionActive = !!session;
      if (session) {
        this.entries = session.history;
        this.providerLang = session.source_lang;
        this.patientLang = session.target_lang;
      }
    } catch {
      // No session yet — normal on first open.
    }
  }

  /** Ensure a backend session exists before an utterance. Sessions die with
   *  the app process, so after a restart the first capture/typed utterance
   *  starts one on demand instead of failing with "no session". */
  private async ensureSession(): Promise<boolean> {
    if (this.sessionActive) return true;
    if (!this.providerLang || !this.patientLang) {
      this.error = 'Pick both languages before translating.';
      return false;
    }
    try {
      await translationApi.startSession(this.patientLang, this.providerLang);
      this.sessionActive = true;
      return true;
    } catch (e) {
      this.error = formatError(e) || 'Could not start the translation session';
      return false;
    }
  }

  /** Start (or restart with new languages) the backend session. The
   *  component owns the "history exists → confirm" decision. */
  async restartSession(providerLang: string, patientLang: string) {
    if (this._busy) return;
    this._busy = true;
    try {
      await translationApi.startSession(patientLang, providerLang);
      this.providerLang = providerLang;
      this.patientLang = patientLang;
      this.sessionActive = true;
      this.entries = [];
      this.error = null;
    } catch (e) {
      this.error = formatError(e) || 'Could not start the translation session';
    } finally {
      this._busy = false;
    }
  }

  private startTimer() {
    this.clearTimer();
    this.timer = setInterval(() => {
      this.elapsed += 1;
      this.checkAutoStop();
    }, 1000);
  }

  /** A single conversational utterance should never run for minutes — a
   *  forgotten tap would otherwise write an ever-growing WAV with no UI to
   *  stop it (the tab can be hidden while the singleton store keeps
   *  capturing). At the cap the capture is stopped and processed like a
   *  manual tap. Extracted from the timer tick for testability. */
  checkAutoStop() {
    if (this.phase === 'recording' && !this._busy && this.elapsed >= MAX_UTTERANCE_SECS) {
      this.setNotice('Capture stopped automatically — utterances are capped at 2 minutes.');
      void this.stopCapture();
    }
  }

  private clearTimer() {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  private async subscribeWaveform() {
    this.teardownWaveform();
    this.waveformUnlisten = await listen<number[]>('waveform-data', (event) => {
      this.waveform = [...this.waveform, ...event.payload].slice(-256);
    });
  }

  private teardownWaveform() {
    this.waveformUnlisten?.();
    this.waveformUnlisten = null;
  }

  /** Tap-to-talk toggle: starts a capture for `speaker`, or stops the
   *  in-flight one and processes it (transcribe → translate → append). */
  async capture(speaker: TranslationSpeaker) {
    if (this.phase === 'recording') {
      await this.stopCapture();
      return;
    }
    // Language requirements live in ensureSession() so the user gets the
    // "Pick both languages" hint instead of a silent no-op.
    if (this.phase !== 'idle' || this._busy) return;
    this._busy = true;
    try {
      // A session is required for the STT language hint and translation
      // direction — start one on demand (sessions die with the app).
      if (!(await this.ensureSession())) return;
      await this.subscribeWaveform();
      await translationApi.captureStart(speaker);
      this.activeSpeaker = speaker;
      this.phase = 'recording';
      this.elapsed = 0;
      this.waveform = [];
      this.error = null;
      this.startTimer();
    } catch (e) {
      this.teardownWaveform();
      this.handleError(e, 'Could not start the microphone');
    } finally {
      this._busy = false;
    }
  }

  private async stopCapture() {
    if (this._busy) return;
    this._busy = true;
    this.clearTimer();
    this.teardownWaveform();
    // Optimistically advance the phase; the backend's progress events
    // ("transcribing" → "translating") refine it from here.
    this.phase = 'transcribing';
    this.waveform = [];
    try {
      const result = await translationApi.captureStop();
      if (result.entry) {
        this.entries = [...this.entries, result.entry];
        this.error = null;
      } else if (result.note) {
        // Expected unusable capture (tap too short, silence, nothing
        // heard) — a retry hint, not a failure.
        this.setNotice(result.note);
      }
    } catch (e) {
      this.handleError(e, 'Could not process that utterance');
    } finally {
      this.phase = 'idle';
      this.activeSpeaker = null;
      this._busy = false;
    }
  }

  /** Typed-input fallback: translate `text` as if `speaker` said it. */
  async submitText(speaker: TranslationSpeaker, text: string) {
    const trimmed = text.trim();
    if (!trimmed || this.phase !== 'idle' || this._busy) return;
    this._busy = true;
    try {
      if (!(await this.ensureSession())) return;
      this.phase = 'translating';
      const entry = await translationApi.textUtterance(speaker, trimmed);
      this.entries = [...this.entries, entry];
      this.error = null;
    } catch (e) {
      this.handleError(e, 'Could not translate that text');
    } finally {
      this.phase = 'idle';
      this._busy = false;
    }
  }

  /** Clear the conversation (component confirms when history exists). */
  async clear() {
    // Busy guard: a clear racing an in-flight capture start (or utterance)
    // is exactly the window the backend's session guards reject — fail
    // early client-side instead of surfacing that error.
    if (this._busy) return;
    if (this.phase === 'recording') {
      // Stop the in-flight capture first so the backend flag is released.
      await this.stopCapture();
    }
    try {
      await translationApi.clearSession();
    } catch (e) {
      this.error = formatError(e) || 'Could not clear the session';
      return;
    }
    this.sessionActive = false;
    this.entries = [];
    this.error = null;
  }

  /** Export the conversation as a plain-text transcript. */
  async exportText(): Promise<string | null> {
    try {
      return await translationApi.exportSession();
    } catch (e) {
      this.error = formatError(e) || 'Could not export the conversation';
      return null;
    }
  }

  /** Speak an entry's translated text aloud in its target language
   *  (patient hears the provider's words in their language, and vice
   *  versa). Fire-and-forget — the OS engine queues utterances. */
  speak(entry: TranslationEntry) {
    translationApi.speak(entry.translated, entry.target_lang).catch((e) => {
      this.error = formatError(e) || 'Speech playback failed';
    });
  }

  private handleError(e: unknown, fallback: string) {
    if (e instanceof OfflineCancelled) {
      // The shared retry dialog already explained; don't double-report.
      this.phase = 'idle';
      this.activeSpeaker = null;
      return;
    }
    this.error = formatError(e) || fallback;
  }

  destroy() {
    this.clearTimer();
    this.teardownWaveform();
    this.progressUnlisten?.();
    this.progressUnlisten = null;
    if (this.noticeTimer) clearTimeout(this.noticeTimer);
    this.noticeTimer = null;
    this.notice = null;
    // Release the re-entrancy guard and session flag too — destroy()
    // doubles as the test reset for this singleton, and a test that fails
    // mid-await would otherwise wedge _busy true for every later test.
    this._busy = false;
    this.sessionActive = false;
  }
}

export const translation = new TranslationStore();
