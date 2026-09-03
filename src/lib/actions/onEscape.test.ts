// @vitest-environment jsdom
/**
 * onEscape — contract tests for the handled/not-handled split. The action
 * stops propagation (preventDefault + stopImmediatePropagation) ONLY when
 * the callback returns true; the old unconditional stop let a
 * mounted-but-closed dialog swallow Escape so the Settings modal behind
 * it could never be Escape-closed.
 *
 * Events dispatched directly on window run listeners in registration
 * order (AT_TARGET), so registering the action before the would-be
 * victim listener mirrors production, where the action's capture phase
 * precedes Modal/ConfirmDialog's bubble-phase window listeners.
 */
import { describe, it, expect, vi } from 'vitest';
import { onEscape } from './onEscape';

describe('onEscape action', () => {
  it('calls the callback when Escape is pressed', () => {
    const cb = vi.fn(() => true);
    const node = window;
    const action = onEscape(node, cb);

    node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));

    expect(cb).toHaveBeenCalledTimes(1);
    action.destroy?.();
  });

  it('does not call the callback for non-Escape keys', () => {
    const cb = vi.fn(() => true);
    const node = window;
    const action = onEscape(node, cb);

    node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }));

    expect(cb).not.toHaveBeenCalled();
    action.destroy?.();
  });

  it('stopImmediatePropagation prevents other listeners when handled', () => {
    const cb = vi.fn(() => true);
    const otherListener = vi.fn();
    const node = window;

    node.addEventListener('keydown', otherListener);
    const action = onEscape(node, cb);

    node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));

    expect(cb).toHaveBeenCalledTimes(1);
    expect(otherListener).not.toHaveBeenCalled();

    action.destroy?.();
    node.removeEventListener('keydown', otherListener);
  });

  it('passes the keypress through when NOT handled (dialog closed)', () => {
    const cb = vi.fn(() => false);
    const victim = vi.fn();
    const node = window;

    node.addEventListener('keydown', victim);
    const action = onEscape(node, cb);

    node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));

    expect(cb).toHaveBeenCalledTimes(1);
    expect(victim).toHaveBeenCalledTimes(1); // reached the listener behind it

    action.destroy?.();
    node.removeEventListener('keydown', victim);
  });

  it('destroy removes the listener', () => {
    const cb = vi.fn(() => true);
    const node = window;
    const action = onEscape(node, cb);

    action.destroy?.();

    node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));

    expect(cb).not.toHaveBeenCalled();
  });

  it('preventDefault is called on a handled Escape, not an unhandled one', () => {
    const node = window;
    const action = onEscape(node, () => true);

    const handled = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
    const handledSpy = vi.spyOn(handled, 'preventDefault');
    node.dispatchEvent(handled);
    expect(handledSpy).toHaveBeenCalled();
    action.destroy?.();

    const action2 = onEscape(node, () => false);
    const unhandled = new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
    const unhandledSpy = vi.spyOn(unhandled, 'preventDefault');
    node.dispatchEvent(unhandled);
    expect(unhandledSpy).not.toHaveBeenCalled();
    action2.destroy?.();
  });
});
