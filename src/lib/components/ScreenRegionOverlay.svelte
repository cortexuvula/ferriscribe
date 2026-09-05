<script lang="ts">
  /**
   * Screen-region selection overlay (X11/Windows capture path).
   *
   * Mounted INSTEAD of the full app when the webview URL carries
   * `#screen-region-overlay` (see src/main.ts). The whole viewport is a
   * transparent, crosshair-cursor surface spanning the virtual desktop; the
   * user drags a rectangle, and the rect (CSS px, viewport-relative) goes to
   * the backend `screen_region_submit` command, which maps it to physical
   * pixels, hides this window, captures, crops, and OCRs. Esc or right-click
   * cancels.
   *
   * This component intentionally imports NO app CSS, stores, or other
   * components — it must stay transparent and featherweight.
   */
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { normalizeDrag, isRealSelection, type DragPoint, type CssRect } from '../utils/screenRegion';

  let dragging = $state(false);
  let start = $state<DragPoint>({ x: 0, y: 0 });
  let current = $state<DragPoint>({ x: 0, y: 0 });
  let submitted = false;

  const selection = $derived<CssRect | null>(dragging ? normalizeDrag(start, current) : null);

  function onMouseDown(e: MouseEvent) {
    if (e.button !== 0) return;
    dragging = true;
    start = { x: e.clientX, y: e.clientY };
    current = { x: e.clientX, y: e.clientY };
  }

  function onMouseMove(e: MouseEvent) {
    if (!dragging) return;
    current = { x: e.clientX, y: e.clientY };
  }

  function onMouseUp() {
    if (!dragging) return;
    dragging = false;
    void submit(current);
  }

  function onContextMenu(e: MouseEvent) {
    e.preventDefault();
    void submit(null);
  }

  /** Resolve the backend one-shot: a real rect commits, anything else
   *  (Esc, right-click, accidental click) cancels. */
  async function submit(end: DragPoint | null) {
    if (submitted) return;
    submitted = true;
    const rect =
      end !== null ? normalizeDrag(start, end) : null;
    const payload = rect !== null && isRealSelection(rect) ? rect : null;
    try {
      await invoke('screen_region_submit', { rect: payload });
    } catch {
      // Backend already tore the overlay down (timeout) — nothing to do.
    }
  }

  onMount(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') void submit(null);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });
</script>

<svelte:window
  onmousedown={onMouseDown}
  onmousemove={onMouseMove}
  onmouseup={onMouseUp}
  oncontextmenu={onContextMenu}
/>

<div class="region-overlay">
  {#if selection}
    <div
      class="selection"
      style="left:{selection.x}px; top:{selection.y}px; width:{selection.width}px; height:{selection.height}px"
    ></div>
  {/if}
  <div class="hint">Drag to select the region to OCR &mdash; Esc to cancel</div>
</div>

<style>
  .region-overlay {
    position: fixed;
    inset: 0;
    cursor: crosshair;
    background: transparent;
    user-select: none;
  }

  .selection {
    position: fixed;
    border: 1px solid #3b82f6;
    background: rgba(59, 130, 246, 0.15);
    pointer-events: none;
  }

  .hint {
    position: fixed;
    top: 24px;
    left: 50%;
    transform: translateX(-50%);
    padding: 6px 14px;
    border-radius: 6px;
    background: rgba(0, 0, 0, 0.7);
    color: #fff;
    font: 13px/1.4 system-ui, sans-serif;
    pointer-events: none;
  }
</style>
