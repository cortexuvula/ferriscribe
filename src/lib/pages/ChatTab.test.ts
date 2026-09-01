// @vitest-environment jsdom
/**
 * ChatTab — tests for the "New chat" (clear/start-over) control.
 *
 *   - The header bar (and button) only appears once there is something to
 *     clear (messages or documents) — the empty chat stays clean.
 *   - With no documents attached, one click clears immediately.
 *   - With documents attached (destructive: OCR time is lost), a confirm
 *     dialog guards the clear; cancel keeps everything.
 *
 * Markup facts these rely on (kept in sync with the component):
 *   - The button is `<button class="clear-btn">New chat</button>`.
 *   - ConfirmDialog renders its confirm label as a button's text.
 */
import { it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/svelte';
import ChatTab from './ChatTab.svelte';

vi.mock('../api/chat', () => ({
  chatStream: vi.fn(),
  chatSend: vi.fn(),
  chatWithAgent: vi.fn(),
  listAiProviders: vi.fn(),
  setActiveProvider: vi.fn(),
  listModels: vi.fn(),
  chatClearDocuments: vi.fn().mockResolvedValue(undefined),
}));
vi.mock('../api/ocr', () => ({
  ocrDocuments: vi.fn(),
}));
// OcrDropZone registers a native drag-drop listener on mount; jsdom has no
// Tauri window, so stub the webview API.
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: vi.fn().mockResolvedValue(() => {}),
  }),
}));

const { chat } = await import('../stores/chat.svelte');
const { chatClearDocuments } = await import('../api/chat');

beforeEach(() => {
  cleanup();
  chat.clear();
  // AFTER the reset clear — clear() itself drops the backend index
  // best-effort, which must not count toward in-test assertions.
  vi.clearAllMocks();
});

afterEach(cleanup);

it('shows no clear control on an empty chat', () => {
  render(ChatTab);
  expect(screen.queryByRole('button', { name: 'New chat' })).toBeNull();
});

it('clears immediately (no confirm) when no documents are attached', async () => {
  chat.addUserMessage('hello');
  chat.addAssistantMessage('hi');
  render(ChatTab);

  const btn = screen.getByRole('button', { name: 'New chat' });
  await fireEvent.click(btn);

  expect(chat.messages).toHaveLength(0);
  // Store.clear() drops the backend conversation index best-effort.
  expect(chatClearDocuments).toHaveBeenCalled();
});

it('confirms before clearing when documents are attached; cancel keeps them', async () => {
  const { ocrDocuments } = await import('../api/ocr');
  vi.mocked(ocrDocuments).mockResolvedValue([
    { filename: 'chart.pdf', page_count: 12, text: 'chart text' },
  ]);
  await chat.ocr.handleOcrFilesSelected(['/tmp/chart.pdf']);
  await new Promise((r) => setTimeout(r, 0));
  chat.addUserMessage('summarize');
  render(ChatTab);

  await fireEvent.click(screen.getByRole('button', { name: 'New chat' }));

  // Destructive clear is guarded: dialog visible, chat intact.
  expect(screen.getByRole('button', { name: 'Clear chat' })).toBeTruthy();
  expect(chat.messages).toHaveLength(1);
  expect(chat.documents).toHaveLength(1);

  await fireEvent.click(screen.getByRole('button', { name: 'Keep' }));
  expect(chat.messages).toHaveLength(1);
  expect(chat.documents).toHaveLength(1);
  expect(chatClearDocuments).not.toHaveBeenCalled();
});

it('confirming the dialog wipes messages and documents', async () => {
  const { ocrDocuments } = await import('../api/ocr');
  vi.mocked(ocrDocuments).mockResolvedValue([
    { filename: 'chart.pdf', page_count: 12, text: 'chart text' },
  ]);
  await chat.ocr.handleOcrFilesSelected(['/tmp/chart.pdf']);
  await new Promise((r) => setTimeout(r, 0));
  chat.addUserMessage('summarize');
  render(ChatTab);

  await fireEvent.click(screen.getByRole('button', { name: 'New chat' }));
  await fireEvent.click(screen.getByRole('button', { name: 'Clear chat' }));

  expect(chat.messages).toHaveLength(0);
  expect(chat.documents).toHaveLength(0);
  expect(chat.ocr.ocrFiles).toHaveLength(0);
  expect(chatClearDocuments).toHaveBeenCalled();
});
