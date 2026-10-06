// @vitest-environment jsdom
/**
 * ConfirmDialog — ARIA contract (trash-restore D9).
 *
 * Facts pinned:
 *   - Default role is 'alertdialog' (every existing caller unchanged).
 *   - role='dialog' is selectable for message-heavy prompts.
 *   - The dialog is aria-modal, and aria-describedby points at the body
 *     element (whose id is unique per instance, so stacked dialogs don't
 *     cross-reference each other's bodies).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/svelte';
import ConfirmDialog from './ConfirmDialog.svelte';

beforeEach(cleanup);

describe('ConfirmDialog — ARIA', () => {
  it('defaults to role=alertdialog with aria-modal', () => {
    render(ConfirmDialog, {
      open: true,
      title: 'Move recording to Trash?',
      message: 'You can restore this recording from Trash for 30 days.',
      onConfirm: () => {},
      onCancel: () => {},
    });
    const dialog = screen.getByRole('alertdialog');
    expect(dialog.getAttribute('aria-modal')).toBe('true');
  });

  it('role is configurable (dialog) for non-alert prompts', () => {
    render(ConfirmDialog, {
      open: true,
      role: 'dialog',
      title: 'Restore by deletion date',
      message: 'long explanatory copy',
      onConfirm: () => {},
      onCancel: () => {},
    });
    expect(screen.getByRole('dialog')).toBeTruthy();
    expect(screen.queryByRole('alertdialog')).toBeNull();
  });

  it('aria-describedby references the body element', () => {
    render(ConfirmDialog, {
      open: true,
      title: 'T',
      message: 'the described body copy',
      onConfirm: () => {},
      onCancel: () => {},
    });
    const dialog = screen.getByRole('alertdialog');
    const describedBy = dialog.getAttribute('aria-describedby');
    expect(describedBy).toBeTruthy();
    const body = document.getElementById(describedBy!);
    expect(body?.textContent).toBe('the described body copy');
  });

  it('stacked instances get distinct body ids (no cross-referencing)', () => {
    const onConfirm = vi.fn();
    const onCancel = vi.fn();
    render(ConfirmDialog, {
      open: true,
      title: 'First',
      message: 'first body',
      onConfirm,
      onCancel,
    });
    render(ConfirmDialog, {
      open: true,
      title: 'Second',
      message: 'second body',
      onConfirm,
      onCancel,
    });
    const dialogs = screen.getAllByRole('alertdialog');
    const id1 = dialogs[0].getAttribute('aria-describedby');
    const id2 = dialogs[1].getAttribute('aria-describedby');
    expect(id1).not.toBe(id2);
    expect(document.getElementById(id1!)?.textContent).toBe('first body');
    expect(document.getElementById(id2!)?.textContent).toBe('second body');
  });
});
