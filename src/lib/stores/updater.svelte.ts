import { check } from '@tauri-apps/plugin-updater';
import { invoke } from '@tauri-apps/api/core';
import { settings } from './settings.svelte';
import { toasts } from './toasts.svelte';

/// How often to auto-check for updates while the app is running.
const CHECK_INTERVAL_MS = 12 * 60 * 60 * 1000; // 12 hours

type UpdateState = 'idle' | 'checking' | 'available' | 'downloading' | 'installed' | 'error';

class UpdaterStore {
  state = $state<UpdateState>('idle');
  availableVersion = $state<string | null>(null);
  downloadProgress = $state<number>(0);
  errorMessage = $state<string | null>(null);
  /// Set when `relaunch()` fails — the update is installed but the restart
  /// command itself refused or errored. The UI surfaces this inline and via
  /// a toast so the user is never left staring at a dead "Restart now" button.
  restartError = $state<string | null>(null);
  lastCheckedAt = $state<Date | null>(null);
  /// An update has been downloaded and installed but the app has NOT been
  /// relaunched yet — including across a refused restart. This is SEPARATE
  /// from `state` (banner visibility): "Later" after install dismisses the
  /// banner without losing the restart obligation, and an intervening
  /// auto-check (which finds the still-pending version) must not clobber
  /// it. Cleared only by a successful relaunch.
  pendingRestart = $state<string | null>(null);

  private intervalId: ReturnType<typeof setInterval> | null = null;

  /// Check GitHub Releases for a newer version. Safe to call regardless of the
  /// `auto_update_check` setting (manual check is always available). Sets
  /// `state` to `available` if a newer version exists, or back to `idle` if
  /// up-to-date. Errors set `state = 'error'` with a message.
  async checkForUpdate(): Promise<void> {
    if (this.state === 'checking' || this.state === 'downloading') return;
    this.state = 'checking';
    this.errorMessage = null;
    try {
      const update = await check();
      this.lastCheckedAt = new Date();
      if (update?.available) {
        this.availableVersion = update.version;
        // If an update is already installed-but-not-restarted, "available"
        // means the pending one — surface the restart, not a re-download.
        this.state = this.pendingRestart !== null ? 'installed' : 'available';
      } else {
        this.availableVersion = null;
        // No update available per the endpoint, but one is already
        // installed and un-restarted: the restart obligation stands.
        this.state = this.pendingRestart !== null ? 'installed' : 'idle';
      }
    } catch (e) {
      this.availableVersion = null;
      this.state = 'error';
      const raw = e instanceof Error ? e.message : String(e);
      // Common transient error: the release was published (latest.json
      // exists) but not all platform assets are uploaded yet. The user
      // checked for updates during the ~5-15 minute release build window.
      if (raw.includes('fallback platforms') || raw.includes('were found in the response')) {
        this.errorMessage = 'Update assets are still being built. Please try again in a few minutes.';
      } else {
        this.errorMessage = 'Could not check for updates. You may be offline or the update server is unavailable.';
      }
    }
  }

  /// Download + verify signature + install the update. Called when the user
  /// clicks "Download & Install" on the banner. On success, sets
  /// `state = 'installed'` and the UI prompts to relaunch.
  async downloadAndInstall(): Promise<void> {
    if (this.state === 'downloading') return;
    this.state = 'downloading';
    this.downloadProgress = 0;
    this.errorMessage = null;
    try {
      const update = await check();
      if (!update?.available) {
        this.state = 'idle';
        return;
      }
      let totalContentLength = 0;
      let downloaded = 0;
      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case 'Started':
            totalContentLength = event.data.contentLength ?? 0;
            break;
          case 'Progress':
            downloaded += event.data.chunkLength;
            this.downloadProgress = totalContentLength > 0
              ? Math.round((downloaded / totalContentLength) * 100)
              : 0;
            break;
          case 'Finished':
            this.downloadProgress = 100;
            break;
        }
      });
      this.state = 'installed';
      // Track the pending restart independently of banner visibility —
      // see the field doc. Survives dismiss() and intervening checks.
      this.pendingRestart = this.availableVersion;
    } catch (e) {
      this.state = 'error';
      const raw = e instanceof Error ? e.message : String(e);
      if (raw.includes('fallback platforms') || raw.includes('were found in the response')) {
        this.errorMessage = `Update assets are still uploading. Please try again in a few minutes.`;
      } else {
        this.errorMessage = raw;
      }
    }
  }

  /// Relaunch the app after a successful install. Uses the backend's
  /// `restart_app` — NOT the plugin-process `relaunch()`: the plugin path
  /// ends in std::process::exit(0), whose C++ static-destructor run aborts
  /// (ONNX Runtime / kaldi-native-fbank teardown with live worker
  /// threads — a SIGABRT crash report on every update relaunch before the
  /// fix; see src-tauri/src/commands/restart.rs). restart_app runs the
  /// coordinated-shutdown checks first and sets the exit-guard flag so
  /// the process terminates via _exit instead.
  ///
  /// A refused restart (recording active / pending save failed) THROWS
  /// with a typed message so both update surfaces (UpdateBanner,
  /// Settings → About) can show actionable guidance; the "installed"
  /// state is preserved — the update stays applied and the restart can
  /// be retried once the work is resolved. On any failure (refusal or
  /// other error), `restartError` is set and a toast fires so the user
  /// sees actionable feedback instead of a silent console error.
  async relaunch(): Promise<void> {
    this.restartError = null;
    try {
      await invoke('restart_app');
      // Only a successful restart clears the pending-restart obligation.
      this.pendingRestart = null;
    } catch (e) {
      const raw = e instanceof Error ? e.message : String(e);
      this.restartError = `Could not restart automatically: ${raw}. Please save your work and restart FerriScribe manually.`;
      console.error('Failed to relaunch:', e);
      toasts.error(this.restartError);
    }
  }

  /// Dismiss the banner (state → idle) without installing. The next auto-check
  /// or manual check will re-surface the banner if the version is still newer.
  /// A pending restart SURVIVES dismissal — `pendingRestart` is separate from
  /// banner visibility, so "Later" after an install keeps the restart
  /// obligation reachable from Settings → About.
  dismiss(): void {
    if (this.state !== 'downloading') {
      this.state = 'idle';
    }
  }

  /// Start the 12h auto-check interval. Only runs the check if
  /// `settings.state.auto_update_check` is true. Called on app launch and when
  /// the user toggles the setting on.
  startAutoCheck(): void {
    this.stopAutoCheck();
    if (!settings.state.auto_update_check) return;
    // Check immediately on start.
    void this.checkForUpdate();
    this.intervalId = setInterval(() => {
      if (settings.state.auto_update_check) {
        void this.checkForUpdate();
      }
    }, CHECK_INTERVAL_MS);
  }

  /// Stop the auto-check interval. Called when the user toggles the setting off.
  stopAutoCheck(): void {
    if (this.intervalId !== null) {
      clearInterval(this.intervalId);
      this.intervalId = null;
    }
  }
}

export const updater = new UpdaterStore();
