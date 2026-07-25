<script lang="ts">
  import { onDestroy, tick } from 'svelte';
  import { chat, isStreaming } from '../stores/chat.svelte';
  import ChatMessage from '../components/ChatMessage.svelte';

  let input = $state('');
  let messagesEl: HTMLDivElement | undefined = $state();

  onDestroy(() => {
    chat.cancel();
  });

  async function scrollToBottom() {
    await tick();
    if (messagesEl) {
      messagesEl.scrollTop = messagesEl.scrollHeight;
    }
  }

  // Scroll to bottom whenever messages change
  $effect(() => {
    chat.messages.length;
    scrollToBottom();
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
</script>

<div class="chat-tab">
  <div class="messages-area" bind:this={messagesEl}>
    {#if chat.messages.length === 0}
      <div class="welcome">
        <div class="welcome-icon">💬</div>
        <h2>Medical AI Chat</h2>
        <p>Ask questions about your recordings, get medical information, or discuss clinical cases.</p>
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

<style>
  .chat-tab {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .messages-area {
    flex: 1;
    overflow-y: auto;
    padding: 12px 0;
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
