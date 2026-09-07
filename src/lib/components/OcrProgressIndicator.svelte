<script lang="ts">
  /**
   * Floating "recognizing text…" status shown by the Rust side while the
   * vision model runs a screenshot-OCR capture. Mounted INSTEAD of the app
   * shell when the webview URL carries `#ocr-progress` (see src/main.ts).
   * The window itself is the pill — frameless, always-on-top, click-through
   * — so this component centers a spinner and label over an opaque dark
   * page. The PAGE paints the dark background (not just the window):
   * wry applies `background_color` to the webview only with its
   * `transparent` feature, which Tauri enables via `macOSPrivateApi` —
   * off in this project — so on macOS the WKWebView keeps its default
   * opaque WHITE background and the white label would be invisible
   * (v0.76.1). Linux/Windows honor the window color; painting here too
   * keeps every platform identical. It must not import app.css or stores
   * — same featherweight rules as the screen-region overlay.
   */
</script>

<svelte:head>
  <style>
    html,
    body {
      /* Same dark as ProgressIndicator's window background_color
         (20, 20, 24) — see the module comment for why the PAGE must
         carry it. Opaque, so the window color behind never matters. */
      background: rgb(20, 20, 24);
      margin: 0;
      overflow: hidden;
    }
  </style>
</svelte:head>

<div class="ocr-progress" role="status" aria-live="polite">
  <span class="spinner" aria-hidden="true"></span>
  <span class="label">Recognizing text&hellip;</span>
</div>

<style>
  .ocr-progress {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 9px;
    font: 13px/1.4 system-ui, -apple-system, sans-serif;
    color: #fff;
    user-select: none;
    animation: pulse 1.4s ease-in-out infinite;
  }

  .spinner {
    width: 14px;
    height: 14px;
    flex-shrink: 0;
    border: 2px solid rgba(255, 255, 255, 0.35);
    border-top-color: #fff;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 0.72;
    }
    50% {
      opacity: 1;
    }
  }
</style>
