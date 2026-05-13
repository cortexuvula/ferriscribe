import { writable, get, type Readable } from 'svelte/store';
import { invoke } from '@tauri-apps/api/core';
import { settings } from './settings';

const POLL_INTERVAL_MS = 10_000;

export type ServiceStatus = 'online' | 'offline' | 'skipped';
export type Overall = 'online' | 'partial' | 'offline' | 'hidden';

export interface EndpointHealthState {
  ai: ServiceStatus;
  stt: ServiceStatus;
  lastCheckedAt: number | null;
  overall: Overall;
}

export interface EndpointHealthStore extends Readable<EndpointHealthState> {
  /** Force an immediate probe (used by settings-change and visibilitychange triggers). */
  probeNow(): Promise<void>;
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

function createEndpointHealthStore(): EndpointHealthStore {
  let timer: ReturnType<typeof setInterval> | null = null;
  let settingsUnsub: (() => void) | null = null;
  let visibilityHandler: (() => void) | null = null;
  let lastProbedKey = '';

  const state = writable<EndpointHealthState>(INITIAL, () => {
    // Runs on first subscribe.
    startPolling();
    return () => stopPolling();
  });

  function probedKey(cfg: any): string {
    return [
      cfg.ai_provider,
      cfg.lmstudio_host, cfg.lmstudio_port,
      cfg.ollama_host, cfg.ollama_port,
      cfg.stt_remote_host, cfg.stt_remote_port, cfg.stt_remote_api_key ?? '',
      cfg.stt_mode,
    ].join('|');
  }

  async function probeAi(cfg: any): Promise<ServiceStatus> {
    const provider = cfg.ai_provider;
    if (provider === 'ollama') {
      if (isLoopbackHost(cfg.ollama_host)) return 'skipped';
      try {
        await invoke('test_ollama_connection', {
          host: cfg.ollama_host,
          port: cfg.ollama_port,
        });
        return 'online';
      } catch {
        return 'offline';
      }
    }
    if (provider === 'lmstudio') {
      if (isLoopbackHost(cfg.lmstudio_host)) return 'skipped';
      try {
        await invoke('test_lmstudio_connection', {
          host: cfg.lmstudio_host,
          port: cfg.lmstudio_port,
        });
        return 'online';
      } catch {
        return 'offline';
      }
    }
    return 'skipped';
  }

  async function probeStt(cfg: any): Promise<ServiceStatus> {
    if (cfg.stt_mode !== 'remote') return 'skipped';
    if (isLoopbackHost(cfg.stt_remote_host)) return 'skipped';
    try {
      await invoke('test_stt_remote_connection', {
        host: cfg.stt_remote_host,
        port: cfg.stt_remote_port,
        apiKey: cfg.stt_remote_api_key,
      });
      return 'online';
    } catch {
      return 'offline';
    }
  }

  async function probeAll(): Promise<void> {
    const cfg = get(settings);
    lastProbedKey = probedKey(cfg);
    const [ai, stt] = await Promise.all([probeAi(cfg), probeStt(cfg)]);
    const overall = computeOverall(ai, stt);
    state.set({ ai, stt, lastCheckedAt: Date.now(), overall });
  }

  function startPolling(): void {
    void probeAll();
    timer = setInterval(() => { void probeAll(); }, POLL_INTERVAL_MS);

    settingsUnsub = settings.subscribe((cfg) => {
      const key = probedKey(cfg);
      if (key !== lastProbedKey && lastProbedKey !== '') {
        void probeAll();
      }
    });

    if (typeof document !== 'undefined') {
      visibilityHandler = () => {
        if (document.visibilityState === 'hidden') {
          if (timer) {
            clearInterval(timer);
            timer = null;
          }
        } else {
          if (!timer) {
            void probeAll();
            timer = setInterval(() => { void probeAll(); }, POLL_INTERVAL_MS);
          }
        }
      };
      document.addEventListener('visibilitychange', visibilityHandler);
    }
  }

  function stopPolling(): void {
    if (timer) { clearInterval(timer); timer = null; }
    if (settingsUnsub) { settingsUnsub(); settingsUnsub = null; }
    if (visibilityHandler && typeof document !== 'undefined') {
      document.removeEventListener('visibilitychange', visibilityHandler);
      visibilityHandler = null;
    }
    lastProbedKey = '';
  }

  return {
    subscribe: state.subscribe,
    probeNow: probeAll,
  };
}

export const endpointHealth = createEndpointHealthStore();
