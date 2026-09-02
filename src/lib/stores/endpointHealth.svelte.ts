import { invoke } from '@tauri-apps/api/core';
import { settings } from './settings.svelte';
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

/** Per-provider probe recipe: which AppConfig fields carry the endpoint,
 * the health-probe path (native API for Ollama, OpenAI-style for the
 * rest), and the keychain slot for the optional bearer. Adding provider
 * support is a one-row change. */
const AI_PROBES: Record<
  string,
  {
    hostField: 'ollama_host' | 'lmstudio_host' | 'omlx_host';
    portField: 'ollama_port' | 'lmstudio_port' | 'omlx_port';
    probePath: string;
    keySlot: string;
    providerName: string;
  }
> = {
  ollama: {
    hostField: 'ollama_host',
    portField: 'ollama_port',
    probePath: '/api/tags',
    keySlot: 'ollama_api_key',
    providerName: 'Ollama',
  },
  lmstudio: {
    hostField: 'lmstudio_host',
    portField: 'lmstudio_port',
    probePath: '/v1/models',
    keySlot: 'lmstudio_api_key',
    providerName: 'LM Studio',
  },
  omlx: {
    hostField: 'omlx_host',
    portField: 'omlx_port',
    probePath: '/v1/models',
    keySlot: 'omlx_api_key',
    providerName: 'oMLX',
  },
};

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
  private consumerCount = 0;

  private probedKey(cfg: AppConfig): string {
    return [
      cfg.ai_provider,
      cfg.lmstudio_host, cfg.lmstudio_port,
      cfg.ollama_host, cfg.ollama_port,
      cfg.omlx_host, cfg.omlx_port,
      cfg.stt_remote_host, cfg.stt_remote_port,
      cfg.stt_mode,
    ].join('|');
  }

  private async probeAi(cfg: AppConfig): Promise<ServiceStatus> {
    const probe = AI_PROBES[cfg.ai_provider];
    if (!probe) return 'skipped';
    const host = cfg[probe.hostField];
    if (isLoopbackHost(host)) return 'skipped';
    // AI api_key is keychain-stored, not a settings field. Fetch it at probe
    // time; treat fetch failure as "no key" so the probe still runs.
    let apiKey: string | undefined = undefined;
    try {
      const key = await invoke<string | null>('get_api_key', {
        provider: probe.keySlot,
      });
      if (key) apiKey = key;
    } catch {
      // Keychain unavailable or no key stored — continue without auth.
    }
    try {
      await invoke('probe_endpoint_reachable', {
        service: 'AiProvider',
        providerName: probe.providerName,
        host,
        port: cfg[probe.portField],
        probePath: probe.probePath,
        apiKey,
      });
      return 'online';
    } catch {
      return 'offline';
    }
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
    const cfg = settings.state;
    this.lastProbedKey = this.probedKey(cfg);
    this.primed = true;
    const [ai, stt] = await Promise.all([this.probeAi(cfg), this.probeStt(cfg)]);
    const overall = computeOverall(ai, stt);
    this.state = { ai, stt, lastCheckedAt: Date.now(), overall };
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

  /**
   * Register an active consumer. Refcounted: the first consumer starts the
   * polling loop + settings subscription + visibility listener; the last
   * consumer tears them down. Returns a stop function the caller can invoke
   * in onDestroy (or just call `stop()` directly). Idempotent if a
   * component mounts/unmounts repeatedly.
   */
  start(): () => void {
    this.consumerCount++;
    if (this.consumerCount === 1) {
      this.startPolling();
    }
    return () => this.stop();
  }

  /** Release a consumer. Pairs with `start()`. */
  stop(): void {
    this.consumerCount = Math.max(0, this.consumerCount - 1);
    if (this.consumerCount === 0) {
      this.stopPolling();
    }
  }

  /** Force an immediate probe (used by settings-change and visibilitychange triggers). */
  async probeNow(): Promise<void> {
    return this.probeAll();
  }
}

export const endpointHealth = new EndpointHealthStore();
