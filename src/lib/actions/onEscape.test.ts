// @vitest-environment jsdom
import { describe, it, expect, vi } from 'vitest';
import { onEscape } from './onEscape';

describe('onEscape action', () => {
  it('calls onclose when Escape is pressed', () => {
    const onclose = vi.fn();
    const node = window;
    const action = onEscape(node, onclose);

    node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));

    expect(onclose).toHaveBeenCalledTimes(1);
    action.destroy?.();
  });

  it('does not call onclose for non-Escape keys', () => {
    const onclose = vi.fn();
    const node = window;
    const action = onEscape(node, onclose);

    node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }));

    expect(onclose).not.toHaveBeenCalled();
    action.destroy?.();
  });

  it('stopImmediatePropagation prevents other listeners on the same node', () => {
    const onclose = vi.fn();
    const otherListener = vi.fn();
    const node = window;

    node.addEventListener('keydown', otherListener);
    const action = onEscape(node, onclose);

    node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));

    expect(onclose).toHaveBeenCalledTimes(1);
    expect(otherListener).not.toHaveBeenCalled();

    action.destroy?.();
    node.removeEventListener('keydown', otherListener);
  });

  it('destroy removes the listener', () => {
    const onclose = vi.fn();
    const node = window;
    const action = onEscape(node, onclose);

    action.destroy?.();

    node.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));

    expect(onclose).not.toHaveBeenCalled();
  });

  it('preventDefault is called on the Escape event', () => {
    const onclose = vi.fn();
    const node = window;
    const action = onEscape(node, onclose);

    const event = new KeyboardEvent('keydown', {
      key: 'Escape',
      bubbles: true,
      cancelable: true,
    });
    const preventDefaultSpy = vi.spyOn(event, 'preventDefault');

    node.dispatchEvent(event);

    expect(preventDefaultSpy).toHaveBeenCalled();
    action.destroy?.();
  });
});
