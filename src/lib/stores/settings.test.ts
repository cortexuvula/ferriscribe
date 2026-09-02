import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { AppConfig } from '../types';

// Mock the API layer so we control load/save outcomes without Tauri.
const mockGetSettings = vi.fn();
const mockSaveSettings = vi.fn();
vi.mock('../api/settings', () => ({
  getSettings: (...args: unknown[]) => mockGetSettings(...args),
  saveSettings: (...args: unknown[]) => mockSaveSettings(...args),
}));

// Import AFTER mocks are registered.
const { settings } = await import('./settings.svelte');

const sampleConfig: AppConfig = {
  ...settings.state, // start from defaults
  ai_provider: 'ollama',
  ollama_host: '192.168.1.100',
  onboarding_completed: true,
};

describe('SettingsStore — subscribe / notify', () => {
  beforeEach(() => {
    mockGetSettings.mockReset();
    mockSaveSettings.mockReset();
    vi.clearAllMocks();
    // Reset singleton flags so each test starts clean.
    settings.loaded = false;
    settings.loadError = false;
  });

  it('subscribe emits current value immediately (store contract)', () => {
    const seen: AppConfig[] = [];
    const unsub = settings.subscribe((v) => seen.push(v));
    expect(seen).toHaveLength(1);
    expect(seen[0]).toBe(settings.state);
    unsub();
  });

  it('subscribe is notified after load() replaces state', async () => {
    mockGetSettings.mockResolvedValue(sampleConfig);
    const seen: AppConfig[] = [];
    const unsub = settings.subscribe((v) => seen.push(v));

    await settings.load();

    // At least 2 emissions: initial + post-load.
    expect(seen.length).toBeGreaterThanOrEqual(2);
    expect(seen[seen.length - 1].ollama_host).toBe('192.168.1.100');
    unsub();
  });

  it('unsubscribe stops notifications', async () => {
    mockGetSettings.mockResolvedValue(sampleConfig);
    const seen: AppConfig[] = [];
    const unsub = settings.subscribe((v) => seen.push(v));
    unsub();

    await settings.load();
    // No additional notification after unsubscribe (beyond the initial emit).
    expect(seen).toHaveLength(1);
  });

  it('multiple subscribers each get notified', async () => {
    mockGetSettings.mockResolvedValue(sampleConfig);
    let countA = 0;
    let countB = 0;
    const unsubA = settings.subscribe(() => countA++);
    const unsubB = settings.subscribe(() => countB++);

    await settings.load();

    expect(countA).toBeGreaterThanOrEqual(2);
    expect(countB).toBeGreaterThanOrEqual(2);
    unsubA();
    unsubB();
  });
});

describe('SettingsStore — load error handling', () => {
  beforeEach(() => {
    mockGetSettings.mockReset();
    vi.clearAllMocks();
    settings.loaded = false;
    settings.loadError = false;
  });

  it('sets loadError on failure without throwing', async () => {
    mockGetSettings.mockRejectedValue(new Error('IPC failed'));
    await settings.load();
    expect(settings.loadError).toBe(true);
    expect(settings.loaded).toBe(false);
  });

  it('clears loadError on successful reload', async () => {
    mockGetSettings.mockRejectedValue(new Error('fail'));
    await settings.load();
    expect(settings.loadError).toBe(true);

    mockGetSettings.mockResolvedValue(sampleConfig);
    await settings.load();
    expect(settings.loadError).toBe(false);
    expect(settings.loaded).toBe(true);
  });
});

describe('SettingsStore — save guard', () => {
  beforeEach(() => {
    mockGetSettings.mockReset();
    mockSaveSettings.mockReset();
    vi.clearAllMocks();
    settings.loaded = false;
    settings.loadError = false;
  });

  it('refuses to save when not loaded', async () => {
    // Ensure not loaded
    settings.loaded = false;
    await settings.save(sampleConfig);
    expect(mockSaveSettings).not.toHaveBeenCalled();
  });
});

describe('SettingsStore — updateField save-queue resurrection', () => {
  beforeEach(() => {
    mockGetSettings.mockReset();
    mockSaveSettings.mockReset();
    vi.clearAllMocks();
    settings.loaded = false;
    settings.loadError = false;
  });

  it('a queued updateField does not resurrect a sibling change whose save failed', async () => {
    // Load a base config from the "server".
    mockGetSettings.mockResolvedValue(sampleConfig);
    await settings.load();

    // First queued save fails; the post-failure reload returns the
    // pristine server config (without the rejected change).
    const saveA = settings.updateField('ollama_host', '10.0.0.9');
    mockSaveSettings.mockRejectedValueOnce(new Error('save a failed'));
    const saveB = settings.updateField('lmstudio_host', '10.0.0.10');

    await expect(saveA).rejects.toThrow('save a failed');
    await saveB;

    expect(mockSaveSettings).toHaveBeenCalledTimes(2);
    // B's payload must be derived from the ROLLED-BACK server state —
    // A's rejected ollama_host must NOT ride along (the old behavior sent
    // B's call-time optimistic snapshot, which contained it).
    const payloadB = mockSaveSettings.mock.calls[1][0] as AppConfig;
    expect(payloadB.ollama_host).toBe(sampleConfig.ollama_host);
    expect(payloadB.lmstudio_host).toBe('10.0.0.10');
  });

  it('a queued save does not resurrect a rolled-back change or clobber a landed updateField', async () => {
    mockGetSettings.mockResolvedValue(sampleConfig);
    await settings.load();

    // saveA intends a whole-config change (two fields) but fails and is
    // rolled back by the reload; updateField lands a different field while
    // a second save sits queued behind it.
    const base = { ...settings.state };
    const saveA = settings.save({ ...base, ollama_host: '10.9.9.9', ai_model: 'x' });
    mockSaveSettings.mockRejectedValueOnce(new Error('save a failed'));
    const fieldB = settings.updateField('temperature', 0.5);
    const saveC = settings.save({ ...settings.state, ai_model: 'y' });

    await expect(saveA).rejects.toThrow('save a failed');
    await fieldB;
    await saveC;

    // C's payload: derived from the drained server state (temperature
    // landed; ollama_host rolled back) plus ONLY C's intended delta
    // (ai_model). The old behavior sent C's call-time snapshot, which
    // contained A's rejected ollama_host and predated B's temperature.
    const payloadC = mockSaveSettings.mock.calls[2][0] as AppConfig;
    expect(payloadC.ollama_host).toBe(sampleConfig.ollama_host);
    expect(payloadC.temperature).toBe(0.5);
    expect(payloadC.ai_model).toBe('y');
  });

  it('successful updateFields chain onto the persisted state', async () => {
    mockGetSettings.mockResolvedValue(sampleConfig);
    await settings.load();
    mockSaveSettings.mockResolvedValue(undefined);

    await settings.updateField('ollama_host', '10.0.0.9');
    await settings.updateField('lmstudio_host', '10.0.0.10');

    expect(mockSaveSettings).toHaveBeenCalledTimes(2);
    const payloadA = mockSaveSettings.mock.calls[0][0] as AppConfig;
    const payloadB = mockSaveSettings.mock.calls[1][0] as AppConfig;
    expect(payloadA.ollama_host).toBe('10.0.0.9');
    // B is A's persisted result + B's delta.
    expect(payloadB.ollama_host).toBe('10.0.0.9');
    expect(payloadB.lmstudio_host).toBe('10.0.0.10');
  });
});
