import { writable, type Readable } from 'svelte/store';

const OPEN_KEY = 'record.sidebar.open';
const WIDTH_KEY = 'record.sidebar.width';
const DEFAULT_WIDTH = 360;
const MIN_WIDTH = 280;
const MAX_WIDTH = 600;

function clamp(n: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, n));
}

function readOpen(): boolean {
  try {
    const v = localStorage.getItem(OPEN_KEY);
    // Open-on-doubt: only the exact string "false" disables.
    return v !== 'false';
  } catch {
    return true;
  }
}

function readWidth(): number {
  try {
    const raw = localStorage.getItem(WIDTH_KEY);
    if (raw === null) return DEFAULT_WIDTH;
    const n = Number(raw);
    if (!Number.isFinite(n) || n <= 0) return DEFAULT_WIDTH;
    return clamp(n, MIN_WIDTH, MAX_WIDTH);
  } catch {
    return DEFAULT_WIDTH;
  }
}

const _open = writable<boolean>(readOpen());
const _width = writable<number>(readWidth());

export const recordSidebar = {
  open: { subscribe: _open.subscribe } as Readable<boolean>,
  width: { subscribe: _width.subscribe } as Readable<number>,

  setOpen(v: boolean) {
    _open.set(v);
    try {
      localStorage.setItem(OPEN_KEY, v ? 'true' : 'false');
    } catch {
      // Persistence best-effort; in-memory value is authoritative.
    }
  },

  setWidth(v: number) {
    const clamped = clamp(Math.round(v), MIN_WIDTH, MAX_WIDTH);
    _width.set(clamped);
    try {
      localStorage.setItem(WIDTH_KEY, String(clamped));
    } catch {
      // Persistence best-effort; in-memory value is authoritative.
    }
  },

  // Exposed for tests and for the resize-helper consumer.
  MIN_WIDTH,
  MAX_WIDTH,
  DEFAULT_WIDTH,
};
