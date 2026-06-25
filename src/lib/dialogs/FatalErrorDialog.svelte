<script lang="ts">
  interface Props {
    message: string;
  }
  const { message }: Props = $props();

  let copied = $state(false);

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(message);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      // Clipboard may be unavailable in some webview contexts; the user can
      // still select the text manually from the <pre> block below.
    }
  }

  function handleQuit() {
    // plugin-process is not currently bundled; closing the window is the
    // simplest portable shutdown signal. The user can then restart FerriScribe.
    window.close();
  }
</script>

<div class="fatal-overlay">
  <div class="fatal-dialog">
    <div class="fatal-icon" aria-hidden="true">⚠️</div>
    <h2>FerriScribe couldn't start</h2>
    <p>
      A critical error occurred during startup. This is usually caused by a
      corrupted database, a failed migration, or a file-system permissions
      problem.
    </p>
    <p>Your recordings are safe on disk — restarting may resolve transient issues.</p>

    <details>
      <summary>Technical detail</summary>
      <pre class="error-detail">{message}</pre>
      <button class="btn-copy" onclick={handleCopy}>
        {copied ? 'Copied' : 'Copy to clipboard'}
      </button>
    </details>

    <div class="hint">
      If this keeps happening, check your disk permissions or restore from a
      backup, then restart FerriScribe.
    </div>

    <div class="actions">
      <button class="btn-primary" onclick={handleQuit}>Quit</button>
    </div>
  </div>
</div>

<style>
  .fatal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 9999;
    padding: 24px;
  }

  .fatal-dialog {
    background: var(--bg-primary, #1a1a1a);
    color: var(--text-primary, #e5e5e5);
    border-radius: 8px;
    padding: 28px;
    max-width: 560px;
    width: 100%;
    box-shadow: 0 20px 50px rgba(0, 0, 0, 0.4);
    text-align: center;
  }

  .fatal-icon {
    font-size: 48px;
    margin-bottom: 12px;
  }

  h2 {
    font-size: 20px;
    font-weight: 600;
    margin: 0 0 12px;
  }

  p {
    font-size: 14px;
    line-height: 1.5;
    color: var(--text-secondary, #a0a0a0);
    margin: 8px 0;
  }

  details {
    text-align: left;
    margin: 16px 0;
    background: rgba(220, 38, 38, 0.08);
    border-radius: 6px;
    padding: 10px 14px;
  }

  summary {
    cursor: pointer;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary, #a0a0a0);
  }

  .error-detail {
    font-family: var(--font-mono, monospace);
    font-size: 12px;
    line-height: 1.5;
    color: var(--text-primary, #e5e5e5);
    background: rgba(0, 0, 0, 0.25);
    padding: 10px;
    border-radius: 4px;
    white-space: pre-wrap;
    word-break: break-word;
    margin: 10px 0 8px;
    max-height: 200px;
    overflow-y: auto;
  }

  .btn-copy {
    font-size: 12px;
    padding: 4px 10px;
    background: none;
    border: 1px solid var(--border, #444);
    border-radius: 4px;
    color: var(--text-secondary, #a0a0a0);
    cursor: pointer;
  }

  .btn-copy:hover {
    color: var(--text-primary, #e5e5e5);
    border-color: var(--accent, #3b82f6);
  }

  .hint {
    font-size: 12px;
    color: var(--text-muted, #7a7a7a);
    margin-top: 16px;
    line-height: 1.5;
  }

  .actions {
    display: flex;
    gap: 8px;
    justify-content: center;
    margin-top: 20px;
  }

  .btn-primary {
    padding: 10px 28px;
    border-radius: 6px;
    border: none;
    cursor: pointer;
    font-size: 14px;
    font-weight: 500;
    background-color: var(--accent, #3b82f6);
    color: white;
    transition: opacity 0.15s ease;
  }

  .btn-primary:hover {
    opacity: 0.9;
  }
</style>
