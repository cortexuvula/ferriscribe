import { invoke } from '@tauri-apps/api/core';
import { invokeWithOfflineHandling } from './invokeWithOfflineHandling';

/** A language supported for translation (mirrors `Language` in
 *  crates/core/src/traits/translation.rs — BCP-47 base code + display name). */
export interface TranslationLanguage {
  code: string;
  name: string;
}

/** Who spoke an utterance (mirrors `Speaker` in
 *  crates/translation/src/session.rs, serde snake_case). */
export type TranslationSpeaker = 'provider' | 'patient';

/** One translated utterance (mirrors `TranslationEntry` in
 *  crates/translation/src/session.rs). */
export interface TranslationEntry {
  original: string;
  translated: string;
  source_lang: string;
  target_lang: string;
  timestamp: string;
  speaker: TranslationSpeaker;
}

/** The active conversation session (mirrors `TranslationSession`). */
export interface TranslationSessionInfo {
  source_lang: string;
  target_lang: string;
  history: TranslationEntry[];
  mode: 'bidirectional' | 'one_way';
  created_at: string;
}

/** Result of stopping an utterance capture (mirrors `CaptureStopResult` in
 *  commands/translation.rs). `entry: null` + `note` marks an EXPECTED
 *  unusable capture (mistimed tap, silence, nothing transcribed) — render
 *  as a soft auto-dismissing notice, not an error. */
export interface CaptureStopResult {
  entry: TranslationEntry | null;
  note: string | null;
}

export function supportedLanguages(): Promise<TranslationLanguage[]> {
  return invoke('translation_supported_languages');
}

export function startSession(
  patientLang: string,
  providerLang: string
): Promise<TranslationSessionInfo> {
  return invoke('translation_start_session', {
    patientLang,
    providerLang,
  });
}

export function getSession(): Promise<TranslationSessionInfo | null> {
  return invoke('translation_get_session');
}

export function clearSession(): Promise<void> {
  return invoke('translation_clear_session');
}

export function exportSession(): Promise<string> {
  return invoke('translation_export_session');
}

export function captureStart(speaker: TranslationSpeaker): Promise<void> {
  return invokeWithOfflineHandling('translation_capture_start', { speaker });
}

export function captureStop(): Promise<CaptureStopResult> {
  return invokeWithOfflineHandling('translation_capture_stop');
}

export function textUtterance(
  speaker: TranslationSpeaker,
  text: string
): Promise<TranslationEntry> {
  return invokeWithOfflineHandling('translation_text_utterance', { speaker, text });
}

/** Speak text aloud via the local OS speech engine in the given language. */
export function speak(text: string, language: string): Promise<void> {
  return invoke('translation_speak', { text, language });
}
