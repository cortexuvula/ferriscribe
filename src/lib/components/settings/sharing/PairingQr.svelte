<script lang="ts">
  import QRCode from 'qrcode';
  export let payload: string;
  let canvas: HTMLCanvasElement;
  $: if (canvas && payload) QRCode.toCanvas(canvas, payload, { width: 240 });
  $: code = payload.match(/[?&]code=(\d{6})/)?.[1] ?? '';
</script>
<div class="qr-block">
  <canvas bind:this={canvas}></canvas>
  {#if code}
    <div class="code-display" aria-label="Pairing code">
      <span class="code-label">Pairing code</span>
      <span class="code-digits">{code}</span>
    </div>
  {/if}
  {#if payload}
    <details class="link-details">
      <summary>Pairing link</summary>
      <code class="payload">{payload}</code>
    </details>
  {/if}
</div>
<style>
  .qr-block { display: flex; flex-direction: column; align-items: flex-start; gap: 0.75rem; }
  .code-display {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }
  .code-label {
    font-size: 0.8rem;
    color: var(--text-muted, #888);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .code-digits {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 2rem;
    font-weight: 700;
    letter-spacing: 0.2em;
  }
  .link-details summary {
    font-size: 0.85rem;
    color: var(--text-muted, #888);
    cursor: pointer;
  }
  .payload { display: block; font-size: 0.75rem; word-break: break-all; margin-top: 0.25rem; }
</style>
