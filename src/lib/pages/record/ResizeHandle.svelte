<script lang="ts">
  type Props = {
    onResize: (deltaPx: number) => void;
    onResizeEnd: () => void;
  };
  let { onResize, onResizeEnd }: Props = $props();

  let startX = 0;
  let dragging = false;

  function pointerdown(ev: PointerEvent) {
    dragging = true;
    startX = ev.clientX;
    (ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId);
    ev.preventDefault();
  }

  function pointermove(ev: PointerEvent) {
    if (!dragging) return;
    const delta = ev.clientX - startX;
    if (delta !== 0) {
      onResize(delta);
      startX = ev.clientX;
    }
  }

  function pointerup(ev: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    try {
      (ev.currentTarget as HTMLElement).releasePointerCapture(ev.pointerId);
    } catch {
      // Capture may already have been released; ignore.
    }
    onResizeEnd();
  }
</script>

<div
  class="resize-handle"
  role="separator"
  aria-orientation="vertical"
  aria-label="Resize patient context sidebar"
  onpointerdown={pointerdown}
  onpointermove={pointermove}
  onpointerup={pointerup}
  onpointercancel={pointerup}
></div>

<style>
  .resize-handle {
    width: 6px;
    cursor: col-resize;
    background: var(--border);
    flex: 0 0 6px;
    user-select: none;
    touch-action: none;
  }
  .resize-handle:hover {
    background: var(--accent);
  }
</style>
