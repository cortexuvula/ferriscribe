import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { settings } from './settings.svelte';

/// How often to auto-check for updates while the app is running.
const CHECK_INTERVAL_MS = 12 * 60 * 60 * 1000; // 12 hours

type UpdateState = 'idle' | 'checking' | 'available' | 'downloading' | 'installed' | 'error';

class UpdaterStore {
  state = $state<UpdateState>('idle');
  availableVersion = $state<string | null>(null);
  downloadProgress = $state<number>(0);
  errorMessage = $state<string | null>(null);
  lastCheckedAt = $state<Date | null>(null);

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
        this.state = 'available';
      } else {
        this.availableVersion = null;
        this.state = 'idle';
      }
    } catch (e) {
      this.state = 'error';
      this.errorMessage = e instanceof Error ? e.message : String(e);
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
    } catch (e) {
      this.state = 'error';
      this.errorMessage = e instanceof Error ? e.message : String(e);
    }
  }

  /// Relaunch the app after a successful install.
  async relaunch(): Promise<void> {
    try {
      await relaunch();
    } catch (e) {
      console.error('Failed to relaunch:', e);
    }
  }

  /// Dismiss the banner (state → idle) without installing. The next auto-check
  /// or manual check will re-surface the banner if the version is still newer.
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
