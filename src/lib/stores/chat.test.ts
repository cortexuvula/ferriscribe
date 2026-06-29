import { describe, it, expect, beforeEach, vi } from 'vitest';

// Mock the Tauri event + API modules
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(vi.fn()),
}));
vi.mock('../api/chat', () => ({
  chatSend: vi.fn(),
  chatStream: vi.fn(),
  chatWithAgent: vi.fn(),
  listAiProviders: vi.fn(),
  setActiveProvider: vi.fn(),
  listModels: vi.fn(),
}));
vi.mock('../api/logging', () => ({
  log: { info: vi.fn(), warn: vi.fn(), error: vi.fn() },
}));
vi.mock('../api/invokeWithOfflineHandling', () => ({
  OfflineCancelled: class extends Error {},
  invokeWithOfflineHandling: vi.fn(),
}));
vi.mock('../types/errors', () => ({
  formatError: vi.fn((e: unknown) => String(e)),
}));

const { chat, isStreaming } = await import('./chat.svelte');

describe('ChatStore', () => {
  beforeEach(() => {
    chat.cancel();
    chat.messages = [];
  });

  it('starts with no messages', () => {
    expect(chat.messages).toHaveLength(0);
    expect(isStreaming.value).toBe(false);
  });

  it('addUserMessage adds a user-role message', () => {
    chat.addUserMessage('Hello');
    expect(chat.messages).toHaveLength(1);
    expect(chat.messages[0].role).toBe('user');
    expect(chat.messages[0].content).toBe('Hello');
    expect(chat.messages[0].id).toMatch(/^msg-/);
  });

  it('addAssistantMessage adds an assistant-role message', () => {
    chat.addAssistantMessage('Hi there', 'chat');
    expect(chat.messages).toHaveLength(1);
    expect(chat.messages[0].role).toBe('assistant');
    expect(chat.messages[0].agent).toBe('chat');
  });

  it('appendToLast appends to the last message content', () => {
    chat.addAssistantMessage('Hello');
    chat.appendToLast(' world');
    expect(chat.messages[0].content).toBe('Hello world');
  });

  it('appendToLast does nothing when messages is empty', () => {
    chat.appendToLast('test');
    expect(chat.messages).toHaveLength(0);
  });

  it('startStreaming adds an empty assistant message and sets streaming flag', () => {
    chat.startStreaming();
    expect(chat.messages).toHaveLength(1);
    expect(chat.messages[0].role).toBe('assistant');
    expect(chat.messages[0].content).toBe('');
    expect(isStreaming.value).toBe(true);
  });

  it('stopStreaming clears the streaming flag', () => {
    chat.startStreaming();
    chat.stopStreaming();
    expect(isStreaming.value).toBe(false);
  });

  it('cancel stops streaming and cleans up', () => {
    chat.startStreaming();
    chat.cancel();
    expect(isStreaming.value).toBe(false);
  });
});
