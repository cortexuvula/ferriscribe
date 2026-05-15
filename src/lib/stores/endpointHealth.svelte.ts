import { get } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { settings } from './settings';
import type { AppConfig } from '../types';

const POLL_INTERVAL_MS = 10_000;

export type ServiceStatus = 'online' | 'offline' | 'skipped';
export type Overall = 'online' | 'partial' | 'offline' | 'hidden';

export interface EndpointHealthState {
  ai: ServiceStatus;
  stt: ServiceStatus;
  lastCheckedAt: number | null;
  overall: Overall;
}

const INITIAL: EndpointHealthState = {
  ai: 'skipped',
  stt: 'skipped',
  lastCheckedAt: null,
  overall: 'hidden',
};

function isLoopbackHost(host: string): boolean {
  if (!host) return true;
  const h = host.trim().toLowerCase();
  const stripped = h.replace(/^\[/, '').replace(/\]$/, '');
  if (stripped === 'localhost' || stripped === '::1') return true;
  return /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(stripped);
}

function computeOverall(ai: ServiceStatus, stt: ServiceStatus): Overall {
  if (ai === 'skipped' && stt === 'skipped') return 'hidden';
  const states = [ai, stt].filter((s): s is 'online' | 'offline' => s !== 'skipped');
  if (states.every((s) => s === 'online')) return 'online';
  if (states.every((s) => s === 'offline')) return 'offline';
  return 'partial';
}

class EndpointHealthStore {
  state = $state<EndpointHealthState>({ ...INITIAL });

  // Private non-reactive internal state
  private timer: ReturnType<typeof setInterval> | null = null;
  private settingsUnsub: (() => void) | null = null;
  private visibilityHandler: (() => void) | null = null;
  private primed = false;
  private lastProbedKey = '';
  private subscriberCount = 0;
  private subscribers = new Set<(value: EndpointHealthState) => void>();

  private probedKey(cfg: AppConfig): string {
    return [
      cfg.ai_provider,
      cfg.lmstudio_host, cfg.lmstudio_port,
      cfg.ollama_host, cfg.ollama_port,
      cfg.stt_remote_host, cfg.stt_remote_port,
      cfg.stt_mode,
    ].join('|');
  }

  private async probeAi(cfg: AppConfig): Promise<ServiceStatus> {
    const provider = cfg.ai_provider;
    if (provider === 'ollama') {
      if (isLoopbackHost(cfg.ollama_host)) return 'skipped';
      // AI api_key is keychain-stored, not a settings field. Fetch it at probe
      // time; treat fetch failure as "no key" so the probe still runs.
      let apiKey: string | undefined = undefined;
      try {
        const key = await invoke<string | null>('get_api_key', {
          provider: 'ollama_api_key',
        });
        if (key) apiKey = key;
      } catch {
        // Keychain unavailable or no key stored — continue without auth.
      }
      try {
        await invoke('probe_endpoint_reachable', {
          service: 'AiProvider',
          providerName: 'Ollama',
          host: cfg.ollama_host,
          port: cfg.ollama_port,
          probePath: '/api/tags',
          apiKey,
        });
        return 'online';
      } catch {
        return 'offline';
      }
    }
    if (provider === 'lmstudio') {
      if (isLoopbackHost(cfg.lmstudio_host)) return 'skipped';
      let apiKey: string | undefined = undefined;
      try {
        const key = await invoke<string | null>('get_api_key', {
          provider: 'lmstudio_api_key',
        });
        if (key) apiKey = key;
      } catch {
        // Keychain unavailable or no key stored — continue without auth.
      }
      try {
        await invoke('probe_endpoint_reachable', {
          service: 'AiProvider',
          providerName: 'LM Studio',
          host: cfg.lmstudio_host,
          port: cfg.lmstudio_port,
          probePath: '/v1/models',
          apiKey,
        });
        return 'online';
      } catch {
        return 'offline';
      }
    }
    return 'skipped';
  }

  private async probeStt(cfg: AppConfig): Promise<ServiceStatus> {
    if (cfg.stt_mode !== 'remote') return 'skipped';
    if (isLoopbackHost(cfg.stt_remote_host)) return 'skipped';

    // STT api key is keychain-stored, not a settings field. Fetch it at probe
    // time; treat fetch failure as "no key" so the probe still runs.
    let apiKey: string | undefined = undefined;
    try {
      const key = await invoke<string | null>('get_api_key', {
        provider: 'stt_remote_api_key',
      });
      if (key) apiKey = key;
    } catch {
      // Keychain unavailable or no key stored — continue without auth.
    }

    try {
      await invoke('probe_endpoint_reachable', {
        service: 'RemoteStt',
        providerName: 'Whisper STT',
        host: cfg.stt_remote_host,
        port: cfg.stt_remote_port,
        probePath: '/v1/models',
        apiKey,
      });
      return 'online';
    } catch {
      return 'offline';
    }
  }

  private async probeAll(): Promise<void> {
    const cfg = get(settings);
    this.lastProbedKey = this.probedKey(cfg);
    this.primed = true;
    const [ai, stt] = await Promise.all([this.probeAi(cfg), this.probeStt(cfg)]);
    const overall = computeOverall(ai, stt);
    const next: EndpointHealthState = { ai, stt, lastCheckedAt: Date.now(), overall };
    this.state = next;
    // Notify Svelte-store-compatible subscribers (used by tests and legacy consumers).
    for (const cb of this.subscribers) {
      cb(next);
    }
  }

  private startPolling(): void {
    void this.probeAll();
    this.timer = setInterval(() => { void this.probeAll(); }, POLL_INTERVAL_MS);

    this.settingsUnsub = settings.subscribe((cfg) => {
      const key = this.probedKey(cfg);
      if (this.primed && key !== this.lastProbedKey) {
        void this.probeAll();
      }
    });

    if (typeof document !== 'undefined') {
      this.visibilityHandler = () => {
        if (document.visibilityState === 'hidden') {
          if (this.timer) {
            clearInterval(this.timer);
            this.timer = null;
          }
        } else {
          if (!this.timer) {
            void this.probeAll();
            this.timer = setInterval(() => { void this.probeAll(); }, POLL_INTERVAL_MS);
          }
        }
      };
      document.addEventListener('visibilitychange', this.visibilityHandler);
    }
  }

  private stopPolling(): void {
    if (this.timer) { clearInterval(this.timer); this.timer = null; }
    if (this.settingsUnsub) { this.settingsUnsub(); this.settingsUnsub = null; }
    if (this.visibilityHandler && typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', this.visibilityHandler);
      this.visibilityHandler = null;
    }
    this.primed = false;
    this.lastProbedKey = '';
  }

  /** Svelte-store-compatible subscribe for lifecycle management and legacy consumers. */
  subscribe(cb: (value: EndpointHealthState) => void): () => void {
    // Emit current value immediately (Svelte store contract).
    cb(this.state);
    this.subscribers.add(cb);
    this.subscriberCount++;
    if (this.subscriberCount === 1) {
      // First subscriber — start polling.
      this.startPolling();
    }
    return () => {
      this.subscribers.delete(cb);
      this.subscriberCount--;
      if (this.subscriberCount === 0) {
        // Last subscriber gone — stop polling.
        this.stopPolling();
      }
    };
  }

  /** Force an immediate probe (used by settings-change and visibilitychange triggers). */
  async probeNow(): Promise<void> {
    return this.probeAll();
  }
}

export const endpointHealth = new EndpointHealthStore();
