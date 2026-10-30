// @vitest-environment jsdom
/**
 * Popover placement math (correction batch: Codie S-3 / PARENT-UI-1).
 *
 * The parent UI review measured the calendar popover top-anchored with
 * max-height 380 clipped inside a 390x320 window (rect bottom 565, not
 * scrollable — the .view-panel's overflow:hidden eats it); Codie's
 * 500x430 probe additionally measured it running off the right edge.
 * The correction makes placement viewport-aware: fixed positioning (it
 * must escape clipping ancestors), horizontal clamping, flip above the
 * trigger when below lacks room, and — when NEITHER side fits — clamping
 * to the roomier side with internal scrolling. Never a larger min window.
 */
import { describe, it, expect } from 'vitest';
import {
  computePopoverPlacement,
  POPOVER_MARGIN,
  POPOVER_GAP,
} from './popoverGeometry';

const TRIGGER_IN_FILTER_BAR = { left: 25, right: 190, top: 222, bottom: 258 };
const NATURAL_HEIGHT = 317; // measured on the live synthetic harness

describe('computePopoverPlacement — vertical strategy', () => {
  it('places below and right-aligned when the window has room', () => {
    // A trigger on the RIGHT side of a wide window: the popover
    // right-aligns to the trigger's right edge.
    const rightSideTrigger = { left: 610, right: 775, top: 222, bottom: 258 };
    const g = computePopoverPlacement(rightSideTrigger, NATURAL_HEIGHT, {
      width: 900,
      height: 700,
    });
    expect(g.placement).toBe('below');
    expect(g.top).toBe(rightSideTrigger.bottom + POPOVER_GAP);
    // Right edge aligned to the trigger's right edge (explicit width pin
    // so a NaN could never pass vacuously via Object.is).
    expect(g.width).toBe(322);
    expect(g.left).toBe(rightSideTrigger.right - g.width);
    // Fully on screen, no internal scroll needed.
    expect(g.top + NATURAL_HEIGHT).toBeLessThanOrEqual(700 - POPOVER_MARGIN);
    expect(g.maxHeight).toBeGreaterThanOrEqual(NATURAL_HEIGHT);
  });

  it('parent’s 390x320 case flips above and clamps with internal scrolling — nothing hangs off screen', () => {
    const g = computePopoverPlacement(TRIGGER_IN_FILTER_BAR, NATURAL_HEIGHT, {
      width: 390,
      height: 320,
    });
    // Below has ~44px, above ~204px — above is the roomier side.
    expect(g.placement).toBe('above');
    expect(g.maxHeight).toBeLessThan(NATURAL_HEIGHT); // clamped, scrolls inside
    // The visible box stays INSIDE the viewport (the parent measured
    // bottom 565 in a 320-high window — that shape must be impossible).
    expect(g.top).toBeGreaterThanOrEqual(POPOVER_MARGIN);
    expect(g.top + g.maxHeight).toBeLessThanOrEqual(320 - POPOVER_MARGIN);
    expect(g.maxHeight).toBe(
      TRIGGER_IN_FILTER_BAR.top - POPOVER_GAP - POPOVER_MARGIN,
    );
  });

  it('prefers flipping above when the natural height fits above but not below', () => {
    // 500px tall window: below has ~224px (< 317), above ~204… taller
    // trigger position for a real "fits above" case:
    const trigger = { left: 100, right: 300, top: 360, bottom: 396 };
    const g = computePopoverPlacement(trigger, 317, { width: 500, height: 500 });
    expect(g.placement).toBe('above');
    expect(g.maxHeight).toBeGreaterThanOrEqual(317); // no scroll — it fits
    expect(g.top + 317).toBeLessThanOrEqual(500 - POPOVER_MARGIN);
  });

  it('clamps to the roomier side when NEITHER side fits (extreme short window)', () => {
    const trigger = { left: 40, right: 240, top: 140, bottom: 176 };
    const g = computePopoverPlacement(trigger, NATURAL_HEIGHT, {
      width: 300,
      height: 200,
    });
    // Below: 200-12-182 = 6px; above: 140-6-12 = 122px → above wins.
    expect(g.placement).toBe('above');
    expect(g.top).toBe(POPOVER_MARGIN);
    expect(g.top + g.maxHeight).toBeLessThanOrEqual(200 - POPOVER_MARGIN);
  });
});

describe('computePopoverPlacement — horizontal clamping', () => {
  it('Codie’s 500x430 probe shape: a right-edge trigger cannot push the popover off screen', () => {
    const g = computePopoverPlacement(
      { left: 300, right: 490, top: 60, bottom: 96 },
      NATURAL_HEIGHT,
      { width: 500, height: 430 },
    );
    expect(g.left).toBeGreaterThanOrEqual(POPOVER_MARGIN);
    expect(g.left + g.width).toBeLessThanOrEqual(500 - POPOVER_MARGIN);
  });

  it('a left-edge trigger cannot push the popover off the left either', () => {
    const g = computePopoverPlacement(
      { left: 2, right: 60, top: 60, bottom: 96 },
      NATURAL_HEIGHT,
      { width: 500, height: 430 },
    );
    expect(g.left).toBeGreaterThanOrEqual(POPOVER_MARGIN);
    expect(g.left + g.width).toBeLessThanOrEqual(500 - POPOVER_MARGIN);
  });

  it('narrows the popover itself when the viewport is narrower than the preferred width', () => {
    const g = computePopoverPlacement(
      { left: 10, right: 250, top: 60, bottom: 96 },
      NATURAL_HEIGHT,
      { width: 260, height: 700 },
    );
    expect(g.width).toBe(260 - 2 * POPOVER_MARGIN);
    expect(g.left).toBe(POPOVER_MARGIN);
    expect(g.left + g.width).toBeLessThanOrEqual(260 - POPOVER_MARGIN);
  });
});

describe('computePopoverPlacement — viewport containment property', () => {
  it('never leaves the viewport, whatever the trigger/viewport shape', () => {
    for (const vw of [320, 390, 430, 500, 640, 900, 1360]) {
      for (const vh of [240, 300, 320, 400, 430, 480, 700, 900]) {
        for (const top of [0, 40, Math.floor(vh / 2) - 18, vh - 80]) {
          for (const left of [0, 30, Math.floor(vw / 2), vw - 160]) {
            const trigger = {
              left,
              right: left + 160,
              top,
              bottom: top + 36,
            };
            const g = computePopoverPlacement(trigger, 380, { width: vw, height: vh });
            expect(Number.isFinite(g.left)).toBe(true);
            expect(Number.isFinite(g.top)).toBe(true);
            expect(Number.isFinite(g.width)).toBe(true);
            expect(Number.isFinite(g.maxHeight)).toBe(true);
            expect(g.left).toBeGreaterThanOrEqual(POPOVER_MARGIN);
            expect(g.left + g.width).toBeLessThanOrEqual(vw - POPOVER_MARGIN);
            expect(g.top).toBeGreaterThanOrEqual(POPOVER_MARGIN);
            const visible = Math.min(380, g.maxHeight);
            expect(g.top + visible).toBeLessThanOrEqual(vh - POPOVER_MARGIN);
            // When clamped, the visible height is the roomier side's space.
            if (g.maxHeight < 380) {
              const spaceBelow = vh - POPOVER_MARGIN - (trigger.bottom + POPOVER_GAP);
              const spaceAbove = trigger.top - POPOVER_GAP - POPOVER_MARGIN;
              expect(g.maxHeight).toBe(Math.max(spaceBelow, spaceAbove));
            }
          }
        }
      }
    }
  });
});
