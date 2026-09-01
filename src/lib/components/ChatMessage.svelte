<script lang="ts">
  import type { ChatMessage } from '../types';
  import { formatTimestamp } from '../utils/format';

  const { message }: { message: ChatMessage } = $props();

  const isUser = $derived(message.role === 'user');
  const roleName = $derived(
    isUser ? 'You' : message.agent ? message.agent : 'Assistant'
  );
</script>

<div class="chat-message" class:user={isUser} class:assistant={!isUser}>
  <div class="bubble">
    <div class="meta">
      <span class="role">{roleName}</span>
      <span class="time">{formatTimestamp(message.timestamp)}</span>
    </div>
    <div class="content">{message.content}</div>

    {#if message.tool_calls && message.tool_calls.length > 0}
      <div class="tool-calls">
        {#each message.tool_calls as tc, i (i)}
          <span class="tool-badge" title="Duration: {tc.duration_ms}ms">
            ⚙ {tc.tool_name}
          </span>
        {/each}
      </div>
    {/if}
  </div>
</div>

<style>
  .chat-message {
    display: flex;
    margin: 6px 12px;
  }

  .chat-message.user {
    justify-content: flex-end;
  }

  .chat-message.assistant {
    justify-content: flex-start;
  }

  .bubble {
    max-width: 75%;
    border-radius: var(--radius-md);
    padding: 10px 12px;
    font-size: 13px;
    line-height: 1.6;
  }

  .user .bubble {
    background-color: var(--accent);
    color: white;
    border-bottom-right-radius: 2px;
  }

  .assistant .bubble {
    background-color: var(--bg-card);
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-bottom-left-radius: 2px;
  }

  .meta {
    display: flex;
    gap: 8px;
    align-items: baseline;
    margin-bottom: 4px;
  }

  .role {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    opacity: 0.7;
  }

  .time {
    font-size: 10px;
    opacity: 0.5;
  }

  .content {
    white-space: pre-wrap;
    word-break: break-word;
  }

  .tool-calls {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 6px;
  }

  .tool-badge {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    padding: 2px 7px;
    border-radius: 10px;
    font-size: 10px;
    background-color: rgba(0, 0, 0, 0.15);
    border: 1px solid rgba(255, 255, 255, 0.2);
  }

  .assistant .tool-badge {
    background-color: var(--bg-tertiary);
    border-color: var(--border);
    color: var(--text-muted);
  }
</style>
