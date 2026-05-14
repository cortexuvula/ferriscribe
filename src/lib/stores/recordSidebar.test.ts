// @vitest-environment jsdom
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { get } from 'svelte/store';

const OPEN_KEY = 'record.sidebar.open';
const WIDTH_KEY = 'record.sidebar.width';

// Re-import the module under test FRESHLY for each test so module-level
// initialization (which reads localStorage) reflects the current mock state.
async function freshStore() {
  vi.resetModules();
  return await import('./recordSidebar');
}

describe('recordSidebar store', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.restoreAllMocks();
  });

  it('defaults to open=true and width=360 when localStorage is empty', async () => {
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.open)).toBe(true);
    expect(get(recordSidebar.width)).toBe(360);
  });

  it('reads persisted open=false', async () => {
    localStorage.setItem(OPEN_KEY, 'false');
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.open)).toBe(false);
  });

  it('reads any non-"false" value as open (open-on-doubt)', async () => {
    localStorage.setItem(OPEN_KEY, 'malformed');
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.open)).toBe(true);
  });

  it('persists open via setOpen', async () => {
    const { recordSidebar } = await freshStore();
    recordSidebar.setOpen(false);
    expect(get(recordSidebar.open)).toBe(false);
    expect(localStorage.getItem(OPEN_KEY)).toBe('false');
    recordSidebar.setOpen(true);
    expect(localStorage.getItem(OPEN_KEY)).toBe('true');
  });

  it('reads persisted width within [280, 600] verbatim', async () => {
    localStorage.setItem(WIDTH_KEY, '420');
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.width)).toBe(420);
  });

  it('clamps too-small persisted width up to 280', async () => {
    localStorage.setItem(WIDTH_KEY, '100');
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.width)).toBe(280);
  });

  it('clamps too-large persisted width down to 600', async () => {
    localStorage.setItem(WIDTH_KEY, '9999');
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.width)).toBe(600);
  });

  it('falls back to 360 when persisted width is non-numeric', async () => {
    localStorage.setItem(WIDTH_KEY, 'abc');
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.width)).toBe(360);
  });

  it('falls back to 360 when persisted width is zero or negative', async () => {
    localStorage.setItem(WIDTH_KEY, '0');
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.width)).toBe(360);
  });

  it('persists width via setWidth, clamping the value', async () => {
    const { recordSidebar } = await freshStore();
    recordSidebar.setWidth(500);
    expect(get(recordSidebar.width)).toBe(500);
    expect(localStorage.getItem(WIDTH_KEY)).toBe('500');
    recordSidebar.setWidth(50);
    expect(get(recordSidebar.width)).toBe(280);
    expect(localStorage.getItem(WIDTH_KEY)).toBe('280');
    recordSidebar.setWidth(99999);
    expect(get(recordSidebar.width)).toBe(600);
  });

  it('survives localStorage.getItem throwing on init', async () => {
    const spy = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    const { recordSidebar } = await freshStore();
    expect(get(recordSidebar.open)).toBe(true);
    expect(get(recordSidebar.width)).toBe(360);
    spy.mockRestore();
  });

  it('survives localStorage.setItem throwing on update', async () => {
    const { recordSidebar } = await freshStore();
    const spy = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('quota');
    });
    expect(() => recordSidebar.setOpen(false)).not.toThrow();
    expect(get(recordSidebar.open)).toBe(false); // in-memory value still updates
    expect(() => recordSidebar.setWidth(420)).not.toThrow();
    expect(get(recordSidebar.width)).toBe(420);
    spy.mockRestore();
  });
});
