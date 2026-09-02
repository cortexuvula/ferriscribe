import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import * as chatApi from '../api/chat';
import type { ChatMessage, ToolCallRecord } from '../types';
import { formatError } from '../types/errors';
import { OfflineCancelled } from '../api/invokeWithOfflineHandling';
import { useOcr } from '../composables/useOcr.svelte';

/** Conservative token estimate for English clinical text (~4 chars/token). */
export function estimateTokens(chars: number): number {
  return Math.ceil(chars / 4);
}

/**
 * Document-stuffing budget for the conversation. Documents below this
 * (combined) are sent whole; above it the UI warns and the user trims —
 * oversized conversations get retrieval in phase 2 (chart-review mode).
 */
export const CHAT_DOC_BUDGET_TOKENS = 24_000;

function generateId(): string {
  return `msg-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
}

class StreamingStore {
  value = $state<boolean>(false);
}

export const isStreaming = new StreamingStore();

class ChatStore {
  messages = $state<ChatMessage[]>([]);

  // Conversation-scanned documents (drop → OCR). Once-off by design: they
  // live only as long as the conversation — clear() wipes them, a restart
  // wipes them, nothing is persisted or synced.
  readonly ocr = useOcr();

  /** Done-OCR files as chat documents. */
  documents = $derived(
    this.ocr.ocrFiles
      .filter((f) => f.status === 'done' && f.text)
      .map((f) => ({ name: f.filename, content: f.text ?? '' }))
  );

  documentsTokenEstimate = $derived(
    this.documents.reduce((n, d) => n + estimateTokens(d.content.length), 0)
  );

  documentsOverBudget = $derived(this.documentsTokenEstimate > CHAT_DOC_BUDGET_TOKENS);

  /**
   * Chart-review mode: the conversation's documents are too large to stuff
   * whole, so the backend indexes them once and answers from retrieved,
   * cited excerpts. See commands/chat_docs.rs.
   */
  chartReviewMode = $derived(this.documentsOverBudget);

  // Active stream cleanup handles — set during sendMessage, cleared by cancel/cleanup.
  private _tokenUnlisten: UnlistenFn | null = null;
  private _doneUnlisten: UnlistenFn | null = null;
  private _errorUnlisten: UnlistenFn | null = null;
  private _safetyTimeout: ReturnType<typeof setTimeout> | null = null;

  /**
   * The backend stream this store is currently attached to. Chat events are
   * global and workers outlive their listeners (tab switches, superseding
   * sends); every payload carries its `stream_id`, and anything not matching
   * the active id is discarded — otherwise a previous stream's leftover
   * tokens would splice into the new message, and its terminal event would
   * tear the new stream's listeners down early.
   */
  private _activeStreamId: string | null = null;

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

  /** Tear down an active stream: cancel the backend worker, unlisten
   * events, clear timeout, reset flag. */
  cancel() {
    // Tell the backend to stop consuming/eming — without this the worker
    // streams on and a later re-mount would receive its stale tokens.
    if (this._activeStreamId) {
      void invoke('chat_cancel_stream').catch(() => {});
    }
    this._activeStreamId = null;
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

    // This send's stream identity — generated here, echoed by the backend
    // in every payload, used to filter global events below.
    const streamId = `stream-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
    this._activeStreamId = streamId;
    const isMine = (payload: { stream_id?: string } | null | undefined) =>
      payload?.stream_id === streamId;

    let cleaned = false;

    const cleanup = () => {
      if (cleaned) return;
      cleaned = true;
      this._activeStreamId = null;
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
      this._tokenUnlisten = await listen<{ stream_id: string; content: string }>(
        'chat-token',
        (event) => {
          if (!isMine(event.payload)) return; // stale stream draining out
          // The backend emits TokenPayload { content } — an OBJECT, not a bare
          // string. The old `listen<string>` type was compile-time fiction;
          // concatenating the object rendered "[object Object]" per token.
          this.appendToLast(event.payload.content);
          // Reset safety timeout on each token — the stream is still alive.
          if (this._safetyTimeout) clearTimeout(this._safetyTimeout);
          this._safetyTimeout = setTimeout(() => {
            if (!cleaned) {
              this.appendOrOverwriteLast('\n\n(Stream timed out)');
              cleanup();
            }
          }, 5 * 60 * 1000);
        }
      );
      this._doneUnlisten = await listen<{ stream_id: string }>('chat-done', (event) => {
        if (!isMine(event.payload)) return;
        cleanup();
      });
      this._errorUnlisten = await listen<{ stream_id: string; message: string } | string>(
        'chat-error',
        (event) => {
          const payload = event.payload;
          if (typeof payload === 'object' && payload !== null) {
            if (!isMine(payload)) return;
          }
          this.appendOrOverwriteLast(`\n\nError: ${formatError(payload)}`);
          cleanup();
        }
      );

      // Build messages for the API — read current value directly.
      // Filter excludes the empty streaming message (assistant with '' content).
      const apiMessages = this.messages
        .filter(
          (m) =>
            m.role === 'user' || (m.role === 'assistant' && m.content)
        )
        .map((m) => ({ role: m.role, content: m.content }));

      // Attach documents when present (full-documents mode). The backend
      // enforces its own hard cap; the UI's budget gate normally prevents
      // an oversized send before it gets here.
      await chatApi.chatStream(apiMessages, {
        documents: this.documents.length > 0 ? this.documents : undefined,
        streamId,
      });
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
    this.ocr.clearOcr();
    isStreaming.value = false;
    // Drop the backend's conversation index (chart-review mode). Best-effort
    // — a dropped index only costs a rebuild on the next oversized send.
    void chatApi.chatClearDocuments().catch(() => {});
  }
}

export const chat = new ChatStore();
