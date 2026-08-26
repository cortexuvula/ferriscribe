<script lang="ts">
  import { onDestroy, tick } from 'svelte';
  import { chat, isStreaming, CHAT_DOC_BUDGET_TOKENS } from '../stores/chat.svelte';
  import ChatMessage from '../components/ChatMessage.svelte';
  import OcrDropZone from '../components/OcrDropZone.svelte';
  import ConfirmDialog from '../components/ConfirmDialog.svelte';

  let input = $state('');
  let messagesEl: HTMLDivElement | undefined = $state();
  let userNearBottom = $state(true);

  onDestroy(() => {
    chat.cancel();
  });

  async function scrollToBottom() {
    await tick();
    if (messagesEl) {
      messagesEl.scrollTop = messagesEl.scrollHeight;
    }
  }

  function onScroll(e: Event) {
    const el = e.currentTarget as HTMLElement;
    // "Near bottom" = within 100px of the bottom
    userNearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 100;
  }

  // Auto-scroll to bottom on new messages only when the user is already near
  // the bottom — otherwise we'd fight a user who has scrolled up to read.
  $effect(() => {
    chat.messages.length;
    if (userNearBottom) {
      scrollToBottom();
    }
  });

  async function sendMessage() {
    const text = input.trim();
    if (!text || isStreaming.value) return;

    input = '';
    await chat.sendMessage(text);
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendMessage();
    }
  }

  let clearDialogOpen = $state(false);

  /** Clear and start over. Documents make it destructive — a 600-page
   *  chart's OCR time is lost — so confirm when any are attached. */
  function requestClear() {
    if (isStreaming.value) return;
    if (chat.documents.length > 0) {
      clearDialogOpen = true;
    } else {
      chat.clear();
    }
  }
</script>

<div class="chat-tab">
  {#if chat.messages.length > 0 || chat.documents.length > 0}
    <div class="chat-header">
      <span class="chat-header-label">Chat</span>
      <button
        class="clear-btn"
        onclick={requestClear}
        disabled={isStreaming.value}
        title={isStreaming.value ? 'Wait for the reply to finish' : 'Clear the conversation and attached documents'}
      >
        New chat
      </button>
    </div>
  {/if}

  <div class="messages-area" bind:this={messagesEl} onscroll={onScroll}>
    {#if chat.messages.length === 0}
      <div class="welcome">
        <div class="welcome-icon">💬</div>
        <h2>Medical AI Chat</h2>
        <p>
          Drop or attach documents (PDF, DOCX, scans) to ask questions about
          them, or just chat. Nothing is saved — conversations and documents
          are cleared when you leave or clear the chat.
        </p>
      </div>
    {:else}
      {#each chat.messages as msg (msg.id)}
        <ChatMessage message={msg} />
      {/each}

      {#if isStreaming.value}
        <div class="streaming-indicator">
          <span class="dot"></span>
          <span class="dot"></span>
          <span class="dot"></span>
        </div>
      {/if}
    {/if}
  </div>

  <div class="documents-area">
    <OcrDropZone
      ocrFiles={chat.ocr.ocrFiles}
      ocrText={chat.ocr.ocrTextDisplay}
      ocrLoading={chat.ocr.ocrLoading}
      onOcrFilesSelected={chat.ocr.handleOcrFilesSelected}
      onOcrTextChange={chat.ocr.handleOcrTextChange}
      onRemoveOcrFile={chat.ocr.handleRemoveOcrFile}
    />
    {#if chat.documents.length > 0}
      <div class="doc-summary" class:over-budget={chat.documentsOverBudget}>
        {#each chat.documents as d (d.name)}
          <span class="doc-line">• {d.name} (~{(
            d.content.length / 4
          ).toLocaleString()} tokens)</span>
        {/each}
        <span class="doc-total">
          {chat.documents.length}
          {chat.documents.length === 1 ? 'document' : 'documents'} · ~{chat.documentsTokenEstimate.toLocaleString()}
          / {CHAT_DOC_BUDGET_TOKENS.toLocaleString()} tokens
        </span>
        {#if chat.chartReviewMode}
          <span class="doc-mode" role="status">
            Chart review mode — too large to include whole; answers draw on the
            most relevant excerpts of your documents. The first question may
            take a couple of minutes while the documents are indexed.
          </span>
        {/if}
      </div>
    {/if}
  </div>

  <div class="input-area">
    <textarea
      class="chat-input"
      placeholder="Type a message... (Enter to send, Shift+Enter for newline)"
      rows={3}
      bind:value={input}
      onkeydown={handleKeyDown}
      disabled={isStreaming.value}
    ></textarea>
    <button
      class="send-btn"
      onclick={sendMessage}
      disabled={!input.trim() || isStreaming.value}
    >
      {isStreaming.value ? '...' : 'Send'}
    </button>
  </div>
</div>

<ConfirmDialog
  open={clearDialogOpen}
  title="Clear this chat?"
  message="This removes the conversation and its attached documents. OCR'd
    text cannot be recovered — you would have to drop and re-OCR the files."
  confirmLabel="Clear chat"
  cancelLabel="Keep"
  danger
  onConfirm={() => {
    clearDialogOpen = false;
    chat.clear();
  }}
  onCancel={() => (clearDialogOpen = false)}
/>

<style>
  .chat-tab {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .chat-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 16px;
    border-bottom: 1px solid var(--border);
  }

  .chat-header-label {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-secondary);
  }

  .clear-btn {
    font-size: 12px;
    padding: 4px 12px;
    color: var(--text-secondary);
    background-color: var(--bg-hover);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
  }

  .clear-btn:hover:not(:disabled) {
    background-color: var(--bg-primary);
  }

  .clear-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .messages-area {
    flex: 1;
    overflow-y: auto;
    padding: 12px 0;
  }

  .documents-area {
    padding: 0 16px;
    border-top: 1px solid var(--border);
  }

  .doc-summary {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 6px 0 8px;
    font-size: 12px;
    color: var(--text-muted);
  }

  .doc-line {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .doc-total {
    margin-top: 2px;
    color: var(--text-secondary);
  }

  .doc-mode {
    margin-top: 4px;
    color: var(--text-secondary);
    font-style: italic;
  }

  .welcome {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    text-align: center;
    padding: 40px;
    gap: 10px;
    color: var(--text-muted);
  }

  .welcome-icon {
    font-size: 48px;
    margin-bottom: 8px;
  }

  .welcome h2 {
    font-size: 20px;
    font-weight: 600;
    color: var(--text-secondary);
  }

  .welcome p {
    font-size: 13px;
    max-width: 360px;
    line-height: 1.6;
  }

  .streaming-indicator {
    display: flex;
    gap: 4px;
    padding: 8px 16px;
    margin: 4px 12px;
  }

  .dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background-color: var(--text-muted);
    animation: bounce 1.2s ease-in-out infinite;
  }

  .dot:nth-child(2) { animation-delay: 0.2s; }
  .dot:nth-child(3) { animation-delay: 0.4s; }

  @keyframes bounce {
    0%, 80%, 100% { transform: scale(0.7); opacity: 0.5; }
    40% { transform: scale(1); opacity: 1; }
  }

  .input-area {
    display: flex;
    gap: 8px;
    padding: 12px;
    border-top: 1px solid var(--border);
    background-color: var(--bg-secondary);
    flex-shrink: 0;
  }

  .chat-input {
    flex: 1;
    resize: none;
    min-height: 0;
    font-size: 13px;
    line-height: 1.5;
    border-radius: var(--radius-md);
  }

  .send-btn {
    align-self: flex-end;
    padding: 8px 16px;
    background-color: var(--accent);
    color: white;
    border-radius: var(--radius-md);
    font-size: 13px;
    font-weight: 500;
    transition: background-color 0.15s ease;
    white-space: nowrap;
  }

  .send-btn:hover:not(:disabled) {
    background-color: var(--accent-hover);
  }

  .send-btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
