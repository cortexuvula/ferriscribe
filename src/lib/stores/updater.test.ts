// @vitest-environment jsdom
import { describe, it, expect, beforeEach, vi } from 'vitest';

// Mock the Tauri updater + process plugins, and the invoke bridge the
// crash-safe relaunch rides.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => undefined),
}));
vi.mock('@tauri-apps/plugin-updater', () => ({
  check: vi.fn(async () => ({ available: false, version: '', downloadAndInstall: vi.fn() })),
}));
vi.mock('@tauri-apps/plugin-process', () => ({
  relaunch: vi.fn(async () => {}),
}));

// Import AFTER mocks are registered.
const { invoke } = await import('@tauri-apps/api/core');
const { relaunch: pluginRelaunch } = await import('@tauri-apps/plugin-process');
const { updater } = await import('./updater.svelte');
const { settings } = await import('./settings.svelte');

describe('UpdaterStore — relaunch', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(invoke).mockResolvedValue(undefined);
  });

  it('relaunch() rides the backend restart_app command — never the plugin relaunch', async () => {
    // The plugin's relaunch ends in std::process::exit(0), whose C++
    // static-destructor run aborts on this app (ONNX Runtime /
    // kaldi-native-fbank teardown — SIGABRT on every update relaunch before
    // the fix; see src-tauri/src/commands/restart.rs).
    await updater.relaunch();
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('restart_app');
    expect(vi.mocked(pluginRelaunch)).not.toHaveBeenCalled();
  });

  it('relaunch() swallows failures (logged, no throw)', async () => {
    vi.mocked(invoke).mockRejectedValue(new Error('failed to restart the app'));
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    await expect(updater.relaunch()).resolves.toBeUndefined();
    expect(errorSpy).toHaveBeenCalledWith('Failed to relaunch:', expect.any(Error));
    errorSpy.mockRestore();
  });
});

describe('UpdaterStore — dismiss', () => {
  beforeEach(() => {
    updater.state = 'available';
    updater.availableVersion = '99.0.0';
    vi.clearAllMocks();
  });

  it('dismiss() resets state to idle', () => {
    updater.dismiss();
    expect(updater.state).toBe('idle');
  });

  it('dismiss() does nothing while downloading', () => {
    updater.state = 'downloading';
    updater.dismiss();
    expect(updater.state).toBe('downloading');
  });
});

describe('UpdaterStore — stopAutoCheck', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('stopAutoCheck() is safe to call when no interval is set', () => {
    // Should not throw.
    updater.stopAutoCheck();
    expect(true).toBe(true);
  });

  it('stopAutoCheck() called twice is safe', () => {
    updater.stopAutoCheck();
    updater.stopAutoCheck();
    expect(true).toBe(true);
  });
});

describe('UpdaterStore — startAutoCheck gating', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    settings.loaded = true;
    // Reset singleton state — prior describe blocks may have left it non-idle.
    updater.state = 'idle';
    updater.availableVersion = null;
    updater.errorMessage = null;
    updater.lastCheckedAt = null;
    updater.downloadProgress = 0;
    updater.stopAutoCheck();
  });

  it('startAutoCheck() does nothing when auto_update_check is false', async () => {
    settings.state.auto_update_check = false;
    updater.startAutoCheck();
    // Give the immediate check a tick to fire (it should be skipped).
    await new Promise((r) => setTimeout(r, 50));
    expect(updater.state).toBe('idle');
  });

  it('startAutoCheck() triggers an immediate check when auto_update_check is true', async () => {
    settings.state.auto_update_check = true;
    updater.startAutoCheck();
    // The mock check() returns { available: false }, so state goes checking -> idle.
    await vi.waitFor(() => {
      expect(updater.state).toBe('idle');
    });
    expect(updater.lastCheckedAt).not.toBeNull();
    updater.stopAutoCheck();
  });
});
