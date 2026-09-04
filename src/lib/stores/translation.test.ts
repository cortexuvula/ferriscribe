import { describe, it, expect, beforeEach, vi } from 'vitest';

// Handler-capturing event mock (chat.test.ts pattern) so tests can fire
// `waveform-data` / `translation-progress` with realistic payload shapes.
type CapturedHandler = (event: { event: string; payload: unknown }) => void;
const listeners = vi.hoisted(() => new Map<string, CapturedHandler[]>());
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((name: string, handler: CapturedHandler) => {
    if (!listeners.has(name)) listeners.set(name, []);
    listeners.get(name)!.push(handler);
    return Promise.resolve(() => {
      listeners.set(name, (listeners.get(name) || []).filter((h) => h !== handler));
    });
  }),
}));

function emit(name: string, payload: unknown) {
  for (const h of listeners.get(name) ?? []) h({ event: name, payload });
}

vi.mock('../api/translation', () => ({
  supportedLanguages: vi.fn(),
  startSession: vi.fn(),
  getSession: vi.fn(),
  clearSession: vi.fn().mockResolvedValue(undefined),
  exportSession: vi.fn(),
  captureStart: vi.fn().mockResolvedValue(undefined),
  captureStop: vi.fn(),
  textUtterance: vi.fn(),
  speak: vi.fn().mockResolvedValue(undefined),
}));
vi.mock('../api/invokeWithOfflineHandling', () => ({
  OfflineCancelled: class extends Error {},
  invokeWithOfflineHandling: vi.fn(),
}));
vi.mock('../types/errors', () => ({
  formatError: vi.fn((e: unknown) => String(e)),
}));

const { translation } = await import('./translation.svelte');

import { MAX_UTTERANCE_SECS, decideLanguageChange } from './translation.svelte';

describe('decideLanguageChange', () => {
  // Regression for the stuck-tab bug: picking Patient = English while
  // Physician was still English used to silently no-op, leaving the store
  // empty while the selects displayed real languages.
  it('equal picks are invalid with a reason — not a silent none', () => {
    const decision = decideLanguageChange('en', 'en', 0);
    expect(decision.action).toBe('invalid');
    if (decision.action === 'invalid') {
      expect(decision.reason).toContain('must differ');
    }
  });

  it('incomplete pairs are none (the selection is still recorded by the caller)', () => {
    expect(decideLanguageChange('', 'en', 0).action).toBe('none');
    expect(decideLanguageChange('fr', '', 0).action).toBe('none');
  });

  it('valid pair with history requires confirmation', () => {
    expect(decideLanguageChange('fr', 'en', 3).action).toBe('confirm');
  });

  it('valid pair without history applies immediately', () => {
    expect(decideLanguageChange('fr', 'en', 0).action).toBe('apply');
  });
});

function makeEntry(
  speaker: 'provider' | 'patient',
  original: string,
  translated: string
) {
  return {
    original,
    translated,
    source_lang: speaker === 'provider' ? 'en' : 'zh',
    target_lang: speaker === 'provider' ? 'zh' : 'en',
    timestamp: '2026-09-04T12:00:00Z',
    speaker,
  };
}

describe('TranslationStore', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    listeners.clear();
    translation.destroy();
    translation.entries = [];
    translation.phase = 'idle';
    translation.activeSpeaker = null;
    translation.providerLang = 'en';
    translation.patientLang = 'zh';
    translation.languages = [];
    translation.waveform = [];
    translation.elapsed = 0;
    translation.error = null;
  });

  it('init loads the language list and rehydrates the session', async () => {
    const { supportedLanguages, getSession } = await import('../api/translation');
    vi.mocked(supportedLanguages).mockResolvedValue([
      { code: 'en', name: 'English' },
      { code: 'zh', name: 'Chinese (中文)' },
    ]);
    vi.mocked(getSession).mockResolvedValue({
      source_lang: 'en',
      target_lang: 'es',
      history: [makeEntry('provider', 'Hello', 'Hola')],
      mode: 'bidirectional',
      created_at: '2026-09-04T11:00:00Z',
    });

    await translation.init();

    expect(translation.languages).toHaveLength(2);
    expect(translation.providerLang).toBe('en');
    expect(translation.patientLang).toBe('es');
    expect(translation.entries).toHaveLength(1);
  });

  it('capture toggles recording → processing → appended entry', async () => {
    const { captureStart, captureStop } = await import('../api/translation');
    vi.mocked(captureStart).mockResolvedValue(undefined);
    const entry = makeEntry('patient', '你好', 'Hello');
    vi.mocked(captureStop).mockResolvedValue({ entry, note: null });

    // Start: phase flips to recording, speaker recorded.
    await translation.capture('patient');
    expect(translation.phase).toBe('recording');
    expect(translation.activeSpeaker).toBe('patient');
    expect(vi.mocked(captureStart)).toHaveBeenCalledWith('patient');

    // Waveform events land in the meter buffer.
    emit('waveform-data', [0.1, 0.5, 0.2]);
    expect(translation.waveform).toEqual([0.1, 0.5, 0.2]);

    // Stop: entry appended, back to idle.
    await translation.capture('patient');
    expect(translation.phase).toBe('idle');
    expect(translation.activeSpeaker).toBeNull();
    expect(translation.entries).toEqual([entry]);
    // The waveform listener was torn down on stop.
    expect(listeners.get('waveform-data')).toHaveLength(0);
  });

  it('progress events refine the phase during stop', async () => {
    // The progress listener is registered by init() — fire it up with
    // inert mocks first.
    const { supportedLanguages, getSession, captureStart, captureStop } =
      await import('../api/translation');
    vi.mocked(supportedLanguages).mockResolvedValue([]);
    vi.mocked(getSession).mockResolvedValue(null);
    await translation.init();

    vi.mocked(captureStart).mockResolvedValue(undefined);
    let release!: (v: import('../api/translation').CaptureStopResult) => void;
    vi.mocked(captureStop).mockImplementation(
      () => new Promise((res) => (release = res))
    );

    await translation.capture('provider');
    const stopping = translation.capture('provider');
    expect(translation.phase).toBe('transcribing');

    // Backend progress event fires while the stop command is in flight.
    emit('translation-progress', 'translating');
    expect(translation.phase).toBe('translating');

    release({ entry: makeEntry('provider', 'Hello', '你好'), note: null });
    await stopping;
    expect(translation.phase).toBe('idle');
  });

  it('capture start failure surfaces an error and stays idle', async () => {
    const { captureStart } = await import('../api/translation');
    vi.mocked(captureStart).mockRejectedValue(new Error('mic busy'));

    await translation.capture('patient');
    expect(translation.phase).toBe('idle');
    expect(translation.error).toContain('mic busy');
    expect(listeners.get('waveform-data')).toHaveLength(0);
  });

  it('capture stop failure surfaces an error without wedging the phase', async () => {
    const { captureStart, captureStop } = await import('../api/translation');
    vi.mocked(captureStart).mockResolvedValue(undefined);
    vi.mocked(captureStop).mockRejectedValue(new Error('no speech'));

    await translation.capture('patient');
    await translation.capture('patient');
    expect(translation.phase).toBe('idle');
    expect(translation.error).toContain('no speech');
    expect(translation.entries).toHaveLength(0);
  });

  it('submitText appends the translated entry', async () => {
    const { textUtterance } = await import('../api/translation');
    const entry = makeEntry('provider', 'Take two pills', '吃两片药');
    vi.mocked(textUtterance).mockResolvedValue(entry);

    await translation.submitText('provider', 'Take two pills');

    expect(vi.mocked(textUtterance)).toHaveBeenCalledWith('provider', 'Take two pills');
    expect(translation.entries).toEqual([entry]);
    expect(translation.phase).toBe('idle');
  });

  it('submitText rejects empty input and missing languages', async () => {
    await translation.submitText('provider', '   ');
    expect(translation.entries).toHaveLength(0);

    translation.patientLang = '';
    await translation.submitText('provider', 'hello');
    expect(translation.entries).toHaveLength(0);
    expect(translation.error).toContain('Pick both languages');
  });

  it('restartSession resets entries and stores the language pair', async () => {
    const { startSession } = await import('../api/translation');
    vi.mocked(startSession).mockResolvedValue({
      source_lang: 'en',
      target_lang: 'fr',
      history: [],
      mode: 'bidirectional',
      created_at: '2026-09-04T11:00:00Z',
    });
    translation.entries = [makeEntry('provider', 'x', 'y')];

    await translation.restartSession('en', 'fr');

    expect(vi.mocked(startSession)).toHaveBeenCalledWith('fr', 'en');
    expect(translation.providerLang).toBe('en');
    expect(translation.patientLang).toBe('fr');
    expect(translation.entries).toHaveLength(0);
  });

  it('clear empties the conversation via the backend', async () => {
    const { clearSession } = await import('../api/translation');
    translation.entries = [makeEntry('provider', 'x', 'y')];

    await translation.clear();

    expect(vi.mocked(clearSession)).toHaveBeenCalled();
    expect(translation.entries).toHaveLength(0);
  });

  it('capture auto-starts the backend session when none exists', async () => {
    const { startSession, captureStart } = await import('../api/translation');
    vi.mocked(startSession).mockResolvedValue({
      source_lang: 'en',
      target_lang: 'zh',
      history: [],
      mode: 'bidirectional',
      created_at: '2026-09-04T11:00:00Z',
    });
    vi.mocked(captureStart).mockResolvedValue(undefined);

    // Fresh store: no backend session (sessions die with the app process).
    await translation.capture('patient');

    // The session is started on demand with the store's language pair
    // (patient first, provider second — the api's argument order) BEFORE
    // the capture begins.
    expect(vi.mocked(startSession)).toHaveBeenCalledWith('zh', 'en');
    expect(vi.mocked(captureStart)).toHaveBeenCalled();
    expect(translation.phase).toBe('recording');
  });

  it('capture without both languages surfaces a hint and never starts audio', async () => {
    const { captureStart } = await import('../api/translation');
    translation.providerLang = '';

    await translation.capture('patient');

    expect(translation.error).toContain('Pick both languages');
    expect(vi.mocked(captureStart)).not.toHaveBeenCalled();
    expect(translation.phase).toBe('idle');
  });

  it('clear no-ops while a capture start is still resolving', async () => {
    const { captureStart, clearSession } = await import('../api/translation');
    const release = { start: null as null | (() => void) };
    vi.mocked(captureStart).mockImplementation(
      () => new Promise<void>((res) => (release.start = res))
    );

    const starting = translation.capture('patient');
    // Wait until the store is actually parked on the pending captureStart
    // (ensureSession's awaits resolve on earlier microtask ticks).
    await vi.waitFor(() => expect(release.start).not.toBeNull());

    // captureStart is still pending → the store is busy; clear must not
    // race it (this is exactly the backend's session-guard window).
    await translation.clear();
    expect(vi.mocked(clearSession)).not.toHaveBeenCalled();

    release.start!();
    await starting;
    expect(translation.phase).toBe('recording');
  });

  it('expected unusable captures surface as a notice, not an error', async () => {
    const { captureStart, captureStop } = await import('../api/translation');
    vi.mocked(captureStart).mockResolvedValue(undefined);
    vi.mocked(captureStop).mockResolvedValue({
      entry: null,
      note: 'No speech was detected — the microphone picked up silence',
    });

    await translation.capture('patient');
    await translation.capture('patient');

    expect(translation.phase).toBe('idle');
    expect(translation.entries).toHaveLength(0);
    expect(translation.error).toBeNull();
    expect(translation.notice).toContain('No speech was detected');
  });

  it('notices dismiss themselves after a few seconds', async () => {
    vi.useFakeTimers();
    try {
      translation.setNotice('tap too short');
      expect(translation.notice).toBe('tap too short');
      vi.advanceTimersByTime(4100);
      expect(translation.notice).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it('a forgotten capture auto-stops at the cap', async () => {
    const { captureStop } = await import('../api/translation');
    vi.mocked(captureStop).mockResolvedValue({
      entry: null,
      note: null,
    });

    // Simulate a capture that has run past the cap (the timer tick calls
    // checkAutoStop every second).
    translation.phase = 'recording';
    translation.activeSpeaker = 'patient';
    translation.elapsed = MAX_UTTERANCE_SECS;

    translation.checkAutoStop();

    expect(vi.mocked(captureStop)).toHaveBeenCalled();
    expect(translation.notice).toContain('stopped automatically');
  });

  it('speak calls the api with the translated text and target language', async () => {
    const { speak } = await import('../api/translation');
    const entry = makeEntry('provider', 'Hello', '你好');

    translation.speak(entry);

    expect(vi.mocked(speak)).toHaveBeenCalledWith('你好', 'zh');
  });
});
