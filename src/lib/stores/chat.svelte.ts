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

  async sendMessage(content: string) {
    this.addUserMessage(content);
    this.startStreaming();

    let tokenUnlisten: UnlistenFn | null = null;
    let doneUnlisten: UnlistenFn | null = null;
    let errorUnlisten: UnlistenFn | null = null;
    let cleaned = false;

    const cleanup = () => {
      if (cleaned) return;
      cleaned = true;
      if (safetyTimeout) clearTimeout(safetyTimeout);
      tokenUnlisten?.();
      doneUnlisten?.();
      errorUnlisten?.();
      this.stopStreaming();
    };

    // Safety timeout: if chat-done/chat-error never fire (backend crash,
    // stream silently ends), clean up after 5 minutes so chat isn't stuck.
    let safetyTimeout: ReturnType<typeof setTimeout> | null = setTimeout(() => {
      if (!cleaned) {
        this.appendToLast('\n\n(Stream timed out — no response received)');
        cleanup();
      }
    }, 5 * 60 * 1000);

    try {
      tokenUnlisten = await listen<string>('chat-token', (event) => {
        this.appendToLast(event.payload);
        // Reset safety timeout on each token — the stream is still alive.
        if (safetyTimeout) clearTimeout(safetyTimeout);
        safetyTimeout = setTimeout(() => {
          if (!cleaned) {
            this.appendToLast('\n\n(Stream timed out)');
            cleanup();
          }
        }, 5 * 60 * 1000);
      });
      doneUnlisten = await listen('chat-done', () => {
        cleanup();
      });
      errorUnlisten = await listen<{ message: string } | string>('chat-error', (event) => {
        this.appendToLast(`\n\nError: ${formatError(event.payload)}`);
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
      this.appendToLast(`\n\nError: ${formatError(e) || 'Chat failed'}`);
      cleanup();
    }
  }

  clear() {
    this.messages = [];
    isStreaming.value = false;
  }
}

export const chat = new ChatStore();
