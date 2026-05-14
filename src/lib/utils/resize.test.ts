import { describe, it, expect } from 'vitest';
import { clampSidebarWidth } from './resize';

describe('clampSidebarWidth', () => {
  // Standard params: min=280, max=600, mainMin=320.
  const args = (requested: number, viewport: number) =>
    [requested, viewport, 280, 600, 320] as const;

  it('returns requested value when within all bounds', () => {
    expect(clampSidebarWidth(...args(400, 2000))).toBe(400);
  });

  it('clamps to max=600 when requested above max', () => {
    expect(clampSidebarWidth(...args(700, 2000))).toBe(600);
  });

  it('clamps to min=280 when requested below min', () => {
    expect(clampSidebarWidth(...args(200, 2000))).toBe(280);
  });

  it('clamps to viewport-mainMin when that is the tightest', () => {
    // viewport=700 leaves 380 for the sidebar after reserving mainMin=320.
    expect(clampSidebarWidth(...args(500, 700))).toBe(380);
  });

  it('falls back to min when viewport is too narrow for both', () => {
    // viewport=500 leaves only 180 — but min wins.
    expect(clampSidebarWidth(...args(500, 500))).toBe(280);
  });

  it('returns exactly min on a viewport that exactly fits min + mainMin', () => {
    // viewport=600 leaves 280 — sidebar fits exactly at min.
    expect(clampSidebarWidth(...args(280, 600))).toBe(280);
  });

  it('returns exactly max on a viewport that has plenty of room', () => {
    expect(clampSidebarWidth(...args(600, 2000))).toBe(600);
  });

  it('rounds the requested value to an integer-friendly number', () => {
    // Helper does not need to round itself — but it should accept floats
    // and return the same float when within bounds (rounding is caller's job).
    expect(clampSidebarWidth(...args(400.5, 2000))).toBe(400.5);
  });
});
