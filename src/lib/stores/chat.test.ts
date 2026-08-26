import { describe, it, expect, beforeEach, vi } from 'vitest';

// Mock the Tauri event + API modules. The event mock CAPTURES registered
// handlers so tests can fire them with realistic payload SHAPES — the
// previous inert mock (listen: vi.fn()) hid the chat-token payload bug
// where the backend's { content } object was concatenated as
// "[object Object]".
type CapturedHandler = (event: { event: string; payload: unknown }) => void;
const listeners = vi.hoisted(() => new Map<string, CapturedHandler[]>());
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((name: string, handler: CapturedHandler) => {
    if (!listeners.has(name)) listeners.set(name, []);
    listeners.get(name)!.push(handler);
    return Promise.resolve(() => {
      listeners.set(name, (listeners.get(name) || []).filter((h) => h !== handler));
    });
  }),
}));

/** Fire a captured listener the way Tauri would deliver it. */
function emit(name: string, payload: unknown) {
  for (const h of listeners.get(name) ?? []) h({ event: name, payload });
}
vi.mock('../api/ocr', () => ({
  ocrDocuments: vi.fn(),
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
    vi.clearAllMocks();
    listeners.clear();
    chat.cancel();
    chat.messages = [];
    chat.ocr.clearOcr();
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

  // ── sendMessage: the streaming engine (first real coverage) ────────────

  it('sendMessage streams tokens as text — never "[object Object]"', async () => {
    // chatStream resolves when we say so; meanwhile we fire events.
    let releaseStream: (() => void) | undefined;
    const { chatStream } = await import('../api/chat');
    vi.mocked(chatStream).mockImplementation(
      (_msgs, _opts) => new Promise<void>((res) => (releaseStream = res))
    );

    const sending = chat.sendMessage('summarize the lipid trend');
    // Listeners register before the stream call, so wait for the CALL.
    await vi.waitFor(() => expect(releaseStream).toBeInstanceOf(Function));

    // REALISTIC payloads: the backend emits TokenPayload { content } objects.
    emit('chat-token', { content: 'Lipids improved ' });
    emit('chat-token', { content: 'since 2024.' });
    emit('chat-done', { usage: null, finish_reason: 'stop' });
    releaseStream!();
    await sending;

    const last = chat.messages[chat.messages.length - 1];
    expect(last.role).toBe('assistant');
    expect(last.content).toBe('Lipids improved since 2024.');
    expect(last.content).not.toContain('[object Object]');
    expect(isStreaming.value).toBe(false);
  });

  it('sendMessage sends only user and non-empty assistant history', async () => {
    let captured: Array<{ role: string; content: string }> = [];
    const { chatStream } = await import('../api/chat');
    vi.mocked(chatStream).mockImplementation((msgs) => {
      captured = msgs;
      return Promise.resolve();
    });

    chat.addUserMessage('first question');
    chat.addAssistantMessage('first answer');
    await chat.sendMessage('second question');

    // The empty streaming placeholder is excluded; history is included.
    expect(captured.map((m) => m.content)).toEqual([
      'first question',
      'first answer',
      'second question',
    ]);
    expect(captured.every((m) => m.role === 'user' || m.role === 'assistant')).toBe(true);
  });

  it('sendMessage removes the placeholder on OfflineCancelled and stays silent', async () => {
    const { chatStream } = await import('../api/chat');
    const { OfflineCancelled } = await import('../api/invokeWithOfflineHandling');
    vi.mocked(chatStream).mockRejectedValue(new OfflineCancelled('cancel'));

    const before = chat.messages.length;
    await chat.sendMessage('hello');

    // User message only — no placeholder, no error text.
    expect(chat.messages.length).toBe(before + 1);
    expect(chat.messages[chat.messages.length - 1].content).toBe('hello');
    expect(isStreaming.value).toBe(false);
  });

  it('sendMessage surfaces stream errors into the assistant message', async () => {
    const { chatStream } = await import('../api/chat');
    vi.mocked(chatStream).mockRejectedValue(new Error('provider exploded'));

    await chat.sendMessage('hello');

    const last = chat.messages[chat.messages.length - 1];
    expect(last.role).toBe('assistant');
    expect(last.content).toContain('provider exploded');
    expect(isStreaming.value).toBe(false);
  });

  // ── Documents (drop → OCR → attached to the request) ───────────────────

  it('estimateTokens is a conservative chars/4 rounding up', async () => {
    const { estimateTokens, CHAT_DOC_BUDGET_TOKENS } = await import('./chat.svelte');
    expect(estimateTokens(0)).toBe(0);
    expect(estimateTokens(1)).toBe(1);
    expect(estimateTokens(80)).toBe(20);
    expect(CHAT_DOC_BUDGET_TOKENS).toBeGreaterThan(0);
  });

  it('done OCR files become documents and ride along with sendMessage', async () => {
    const { ocrDocuments } = await import('../api/ocr');
    vi.mocked(ocrDocuments).mockResolvedValue([
      { filename: 'consult.pdf', page_count: 3, text: 'Cardiology consult text' },
    ]);
    await chat.ocr.handleOcrFilesSelected(['/tmp/consult.pdf']);
    await new Promise((r) => setTimeout(r, 0));

    expect(chat.documents).toEqual([
      { name: 'consult.pdf', content: 'Cardiology consult text' },
    ]);
    expect(chat.documentsTokenEstimate).toBeGreaterThan(0);

    let capturedDocs: Array<{ name: string; content: string }> | undefined;
    const { chatStream } = await import('../api/chat');
    vi.mocked(chatStream).mockImplementation((_msgs, opts) => {
      capturedDocs = opts?.documents;
      return Promise.resolve();
    });
    await chat.sendMessage('what does the consult say?');

    expect(capturedDocs).toEqual([
      { name: 'consult.pdf', content: 'Cardiology consult text' },
    ]);
  });

  it('no documents means no documents field on the request', async () => {
    const { chatStream } = await import('../api/chat');
    let capturedDocs: unknown = 'sentinel';
    vi.mocked(chatStream).mockImplementation((_msgs, opts) => {
      capturedDocs = opts?.documents;
      return Promise.resolve();
    });
    await chat.sendMessage('plain question');
    expect(capturedDocs).toBeUndefined();
  });

  it('clear() wipes documents along with the conversation', async () => {
    const { ocrDocuments } = await import('../api/ocr');
    vi.mocked(ocrDocuments).mockResolvedValue([
      { filename: 'labs.pdf', page_count: 1, text: 'LDL 3.2' },
    ]);
    await chat.ocr.handleOcrFilesSelected(['/tmp/labs.pdf']);
    await new Promise((r) => setTimeout(r, 0));
    expect(chat.documents.length).toBe(1);

    chat.clear();
    expect(chat.messages).toHaveLength(0);
    expect(chat.documents).toHaveLength(0);
    expect(chat.ocr.ocrFiles).toHaveLength(0);
  });
});
