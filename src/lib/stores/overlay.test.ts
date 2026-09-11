// @vitest-environment jsdom
/**
 * overlay — open-instance stack tests. The stack drives whose Escape is
 * whose across nested overlays, and `anyOverlayOpen` is what global page
 * shortcuts (Record tab's Space-to-toggle) check before acting.
 */
import { describe, expect, it, vi } from 'vitest';
import { pushOverlay, isTopmostOverlay, anyOverlayOpen, trapTabWithin } from './overlay';

function el(): HTMLElement {
  return document.createElement('div');
}

describe('overlay stack', () => {
  it('tracks topmost across nesting and reports any-open', () => {
    const a = el();
    const b = el();
    expect(anyOverlayOpen()).toBe(false);

    const un1 = pushOverlay(a);
    expect(anyOverlayOpen()).toBe(true);
    expect(isTopmostOverlay(a)).toBe(true);
    expect(isTopmostOverlay(b)).toBe(false);

    const un2 = pushOverlay(b);
    expect(isTopmostOverlay(b)).toBe(true);
    expect(isTopmostOverlay(a)).toBe(false); // buried by the nested overlay

    un2();
    expect(isTopmostOverlay(a)).toBe(true); // topmost again after close
    expect(anyOverlayOpen()).toBe(true);

    un1();
    expect(anyOverlayOpen()).toBe(false);
    expect(isTopmostOverlay(a)).toBe(false);
  });

  it('unregistering twice is a no-op', () => {
    const a = el();
    const un = pushOverlay(a);
    un();
    un(); // must not throw or corrupt the stack
    expect(anyOverlayOpen()).toBe(false);
  });
});

describe('trapTabWithin', () => {
  it('includes disclosure summaries after buttons in the focus cycle', () => {
    const root = el();
    const button = document.createElement('button');
    const details = document.createElement('details');
    const summary = document.createElement('summary');
    summary.textContent = 'Help';
    details.append(summary);
    root.append(button, details);
    document.body.append(root);
    for (const control of [button, summary]) Object.defineProperty(control, 'offsetParent', { get: () => root });
    button.focus();
    const tab = new KeyboardEvent('keydown', { key: 'Tab', cancelable: true });
    trapTabWithin(root, tab);
    expect(tab.defaultPrevented).toBe(false);
    summary.focus();
    const wrap = new KeyboardEvent('keydown', { key: 'Tab', cancelable: true });
    trapTabWithin(root, wrap);
    expect(wrap.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(button);
    root.remove();
  });
  it('ignores non-Tab keys', () => {
    const root = el();
    const focusSpy = vi.spyOn(root, 'focus');
    trapTabWithin(root, new KeyboardEvent('keydown', { key: 'Enter' }));
    expect(focusSpy).not.toHaveBeenCalled();
  });

  it('ignores Tab when focus lives outside the root', () => {
    const root = el();
    const inner = el();
    root.append(inner);
    const outside = el();
    document.body.append(outside, root);
    outside.focus();

    const focusSpy = vi.spyOn(inner, 'focus');
    trapTabWithin(root, new KeyboardEvent('keydown', { key: 'Tab' }));
    expect(focusSpy).not.toHaveBeenCalled();

    outside.remove();
    root.remove();
  });
});
