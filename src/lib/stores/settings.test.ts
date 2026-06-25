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
