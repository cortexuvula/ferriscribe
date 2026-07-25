import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import * as chatApi from '../api/chat';
import type { ChatMessage, ToolCallRecord } from '../types';
import { formatError } from '../types/errors';
import { OfflineCancelled } from '../api/invokeWithOfflineHandling';

function generateId(): string {
  return `msg-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
}

class StreamingStore {
  value = $state<boolean>(false);
}

export const isStreaming = new StreamingStore();

class ChatStore {
  messages = $state<ChatMessage[]>([]);

  // Active stream cleanup handles — set during sendMessage, cleared by cancel/cleanup.
  private _tokenUnlisten: UnlistenFn | null = null;
  private _doneUnlisten: UnlistenFn | null = null;
  private _errorUnlisten: UnlistenFn | null = null;
  private _safetyTimeout: ReturnType<typeof setTimeout> | null = null;

  addUserMessage(content: string) {
    const msg: ChatMessage = {
      id: generateId(),
      role: 'user',
      content,
      timestamp: new Date().toISOString(),
    };
    this.messages = [...this.messages, msg];
  }

  addAssistantMessage(
    content: string,
    agent?: string,
    tool_calls?: ToolCallRecord[]
  ) {
    const msg: ChatMessage = {
      id: generateId(),
      role: 'assistant',
      content,
      timestamp: new Date().toISOString(),
      agent,
      tool_calls,
    };
    this.messages = [...this.messages, msg];
  }

  appendToLast(delta: string) {
    if (this.messages.length === 0) return;
    const last = this.messages[this.messages.length - 1];
    const updated: ChatMessage = { ...last, content: last.content + delta };
    this.messages = [...this.messages.slice(0, -1), updated];
  }

  /**
   * Append to the last message, but if its content is currently empty (e.g. a
   * fresh streaming placeholder), replace the content instead of appending.
   * This avoids producing a leading blank line when an error or status notice
   * is the first thing written to the message.
   */
  appendOrOverwriteLast(text: string) {
    if (this.messages.length === 0) return;
    const last = this.messages[this.messages.length - 1];
    const content = last.content === '' ? text : last.content + text;
    const updated: ChatMessage = { ...last, content };
    this.messages = [...this.messages.slice(0, -1), updated];
  }

  startStreaming() {
    const msg: ChatMessage = {
      id: generateId(),
      role: 'assistant',
      content: '',
      timestamp: new Date().toISOString(),
    };
    this.messages = [...this.messages, msg];
    isStreaming.value = true;
  }

  stopStreaming() {
    isStreaming.value = false;
  }

  /** Tear down an active stream: unlisten events, clear timeout, reset flag. */
  cancel() {
    if (this._safetyTimeout) clearTimeout(this._safetyTimeout);
    this._safetyTimeout = null;
    this._tokenUnlisten?.();
    this._tokenUnlisten = null;
    this._doneUnlisten?.();
    this._doneUnlisten = null;
    this._errorUnlisten?.();
    this._errorUnlisten = null;
    this.stopStreaming();
  }

  async sendMessage(content: string) {
    this.cancel();
    this.addUserMessage(content);
    this.startStreaming();

    let cleaned = false;

    const cleanup = () => {
      if (cleaned) return;
      cleaned = true;
      this.cancel();
    };

    // Safety timeout: if chat-done/chat-error never fire (backend crash,
    // stream silently ends), clean up after 5 minutes so chat isn't stuck.
    this._safetyTimeout = setTimeout(() => {
      if (!cleaned) {
        this.appendOrOverwriteLast('\n\n(Stream timed out — no response received)');
        cleanup();
      }
    }, 5 * 60 * 1000);

    try {
      this._tokenUnlisten = await listen<string>('chat-token', (event) => {
        this.appendToLast(event.payload);
        // Reset safety timeout on each token — the stream is still alive.
        if (this._safetyTimeout) clearTimeout(this._safetyTimeout);
        this._safetyTimeout = setTimeout(() => {
          if (!cleaned) {
            this.appendOrOverwriteLast('\n\n(Stream timed out)');
            cleanup();
          }
        }, 5 * 60 * 1000);
      });
      this._doneUnlisten = await listen('chat-done', () => {
        cleanup();
      });
      this._errorUnlisten = await listen<{ message: string } | string>('chat-error', (event) => {
        this.appendOrOverwriteLast(`\n\nError: ${formatError(event.payload)}`);
        cleanup();
      });

      // Build messages for the API — read current value directly.
      // Filter excludes the empty streaming message (assistant with '' content).
      const apiMessages = this.messages
        .filter(
          (m) =>
            m.role === 'user' || (m.role === 'assistant' && m.content)
        )
        .map((m) => ({ role: m.role, content: m.content }));

      await chatApi.chatStream(apiMessages);
    } catch (e) {
      if (e instanceof OfflineCancelled) {
        // Remove the empty streaming placeholder; the dialog already informed the user.
        this.messages = this.messages.slice(0, -1);
        cleanup();
        return;
      }
      this.appendOrOverwriteLast(`\n\nError: ${formatError(e) || 'Chat failed'}`);
      cleanup();
    }
  }

  clear() {
    this.cancel();
    this.messages = [];
    isStreaming.value = false;
  }
}

export const chat = new ChatStore();
