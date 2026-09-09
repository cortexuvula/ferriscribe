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

  it('relaunch() surfaces refusals via restartError and toast (not console-only)', async () => {
    // U1/ui-consultant fix + U7: the old contract swallowed failures with a
    // console.error, stranding the user on "restart required" with no
    // feedback. Refusals (recording active / save failed) must now be
    // caught, set restartError, fire a toast, and keep state 'installed'.
    vi.mocked(invoke).mockRejectedValue(
      new Error('RESTART_REFUSED_RECORDING_ACTIVE: stop the recording first'),
    );
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    updater.state = 'installed';
    updater.pendingRestart = '99.0.0';
    await expect(updater.relaunch()).resolves.toBeUndefined();
    expect(updater.state).toBe('installed');
    expect(updater.restartError).toContain('Could not restart automatically');
    expect(updater.restartError).toContain('RESTART_REFUSED_RECORDING_ACTIVE');
    // pendingRestart survives a refusal — the update is installed, only
    // the restart command failed.
    expect(updater.pendingRestart).toBe('99.0.0');

    const { toasts } = await import('./toasts.svelte');
    expect(toasts.list.length).toBeGreaterThan(0);
    expect(toasts.list[toasts.list.length - 1].type).toBe('error');

    errorSpy.mockRestore();
    toasts.destroy();
  });

  it('relaunch() surfaces non-refusal errors via restartError and toast', async () => {
    vi.mocked(invoke).mockRejectedValue(new Error('failed to restart the app'));
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});

    updater.state = 'installed';
    await expect(updater.relaunch()).resolves.toBeUndefined();
    expect(updater.state).toBe('installed');
    expect(updater.restartError).toContain('Could not restart automatically');
    expect(updater.restartError).toContain('failed to restart the app');

    const { toasts } = await import('./toasts.svelte');
    expect(toasts.list.length).toBeGreaterThan(0);
    expect(toasts.list[toasts.list.length - 1].type).toBe('error');

    errorSpy.mockRestore();
    toasts.destroy();
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
    updater.pendingRestart = null;
    updater.restartError = null;
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

describe('UpdaterStore — pendingRestart (restart obligation survives)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    updater.state = 'idle';
    updater.availableVersion = null;
    updater.errorMessage = null;
    updater.lastCheckedAt = null;
    updater.downloadProgress = 0;
    updater.pendingRestart = null;
    updater.stopAutoCheck();
  });

  it('refusal → Later → check → Settings restart still available', async () => {
    // Install completes, restart is refused, user hits "Later", an
    // auto-check intervenes — the restart obligation must survive all of
    // it and remain restartable from Settings → About.
    updater.state = 'installed';
    updater.availableVersion = '99.0.0';
    updater.pendingRestart = '99.0.0';

    // Refused restart: pendingRestart must NOT clear. The merged
    // contract catches the error (sets restartError + toast) rather
    // than throwing, so assert resolves + state preserved.
    vi.mocked(invoke).mockRejectedValue(
      new Error('RESTART_REFUSED_RECORDING_ACTIVE: stop the recording first'),
    );
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    await expect(updater.relaunch()).resolves.toBeUndefined();
    expect(updater.pendingRestart).toBe('99.0.0');
    expect(updater.restartError).toContain('RESTART_REFUSED_RECORDING_ACTIVE');
    errorSpy.mockRestore();

    // "Later" dismisses the banner…
    updater.dismiss();
    expect(updater.state).toBe('idle');
    // …but the obligation survives.
    expect(updater.pendingRestart).toBe('99.0.0');

    // An intervening auto-check (endpoint still reports the version — and
    // even one that reports nothing new) must not clobber it.
    await updater.checkForUpdate();
    expect(updater.pendingRestart).toBe('99.0.0');
    expect(updater.state).toBe('installed');

    // Settings → About path: a successful restart clears it.
    vi.mocked(invoke).mockResolvedValue(undefined);
    await updater.relaunch();
    expect(updater.pendingRestart).toBeNull();
  });

  it('pendingRestart set only on successful install', async () => {
    // downloadAndInstall failing (e.g. signature failure) must not create
    // a phantom restart obligation.
    const { check } = await import('@tauri-apps/plugin-updater');
    vi.mocked(check).mockResolvedValueOnce({
      available: true,
      version: '99.0.0',
      downloadAndInstall: vi.fn(async () => {
        throw new Error('invalid signature');
      }),
    } as never);
    await updater.downloadAndInstall();
    expect(updater.state).toBe('error');
    expect(updater.pendingRestart).toBeNull();
  });
});
