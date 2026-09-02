// @vitest-environment jsdom
/**
 * ChatMessage — tests for the assistant-response copy button.
 *
 *   - Assistant messages get a Copy button; clicking it writes the message
 *     content to the clipboard (Tauri plugin, mocked) and shows "Copied ✓".
 *   - User messages get no copy button (their text is already at hand).
 *   - Empty assistant messages (e.g. a pending stream placeholder) show no
 *     button — there is nothing to copy.
 */
import { it, expect, vi, describe } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/svelte';
import ChatMessage from './ChatMessage.svelte';
import type { ChatMessage as ChatMessageType } from '../types';

const writeTextMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/plugin-clipboard-manager', () => ({
  writeText: writeTextMock,
}));

function msg(overrides: Partial<ChatMessageType>): ChatMessageType {
  return {
    id: 'm1',
    role: 'assistant',
    content: 'Take the antibiotics with food.',
    timestamp: new Date('2026-09-01T10:00:00Z').toISOString(),
    ...overrides,
  };
}

describe('ChatMessage copy button', () => {
  it('copies the assistant content and confirms', async () => {
    writeTextMock.mockResolvedValue(undefined);
    render(ChatMessage, { props: { message: msg({}) } });

    const btn = screen.getByRole('button', { name: 'Copy response' });
    await fireEvent.click(btn);

    expect(writeTextMock).toHaveBeenCalledWith('Take the antibiotics with food.');
    expect(screen.getByText('Copied ✓')).toBeTruthy();
    cleanup();
  });

  it('renders no copy button on user messages', () => {
    render(ChatMessage, { props: { message: msg({ role: 'user' }) } });
    expect(screen.queryByRole('button', { name: 'Copy response' })).toBeNull();
    cleanup();
  });

  it('renders no copy button on empty assistant content', () => {
    render(ChatMessage, { props: { message: msg({ content: '' }) } });
    expect(screen.queryByRole('button', { name: 'Copy response' })).toBeNull();
    cleanup();
  });
});
