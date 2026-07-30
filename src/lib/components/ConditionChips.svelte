<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { invoke } from '@tauri-apps/api/core';
  import {
    addConditionChip,
    listConditionChips,
    removeConditionChip,
    reorderConditionChips,
  } from '../api/conditions';
  import type { ConditionChip } from '../api/conditions';
  import { settings } from '../stores/settings.svelte';
  import { toasts } from '../stores/toasts.svelte';

  let { onAdd }: { onAdd: (condition: string) => void } = $props();

  // The default list shown while the backend list is loading or when it's
  // empty (fresh install / backend default). Once the backend returns a
  // non-empty list, those values take precedence.
  const DEFAULT_CONDITIONS = [
    'Hypertension',
    'Type 2 diabetes',
    'Hyperlipidemia',
    'Asthma',
    'COPD',
    'Hypothyroidism',
    'Atrial fibrillation',
    'Coronary artery disease',
    'CKD (chronic kidney disease)',
    'GERD',
    'Anxiety',
    'Depression',
    'Osteoarthritis',
    'Obesity',
    'Sleep apnea',
  ];

  // Build default chip objects for display fallback (when not loaded or empty).
  const DEFAULT_CHIPS: ConditionChip[] = DEFAULT_CONDITIONS.map((text, i) => ({
    id: '',
    text,
    updated_at: '',
    deleted_at: null,
    sort_order: i,
  }));

  let chips = $state<ConditionChip[]>([]);
  let loaded = $state(false);
  let adding = $state(false);
  let newCondition = $state('');

  // Hint text "Drag to reorder" is shown until the user's first successful
  // drag, then permanently dismissed via localStorage.
  let showDragHint = $state(
    typeof localStorage !== 'undefined' && !localStorage.getItem('hasDraggedChips')
  );

  // Display defaults until the backend list loads (or if it's empty).
  let displayChips = $derived(loaded && chips.length > 0 ? chips : DEFAULT_CHIPS);

  // Poll handle for periodic chip refresh (cleared on destroy).
  let pollHandle: ReturnType<typeof setInterval> | null = null;

  // Unsubscribe function for the SSE event listener (set up in onMount,
  // invoked in onDestroy). Null until the listener attaches or if it failed.
  let unlistenSSE: (() => void) | null = null;

  // Tracks whether the user made a local mutation (add/remove/reorder) within
  // the last 5s. If the 30s poll detects a remote change while dirtySince is
  // set, we surface a toast instead of silently clobbering their edit.
  let dirtySince = $state<number | null>(null);
  let dirtyTimer: ReturnType<typeof setTimeout> | null = null;

  function markDirty() {
    dirtySince = Date.now();
    if (dirtyTimer) clearTimeout(dirtyTimer);
    dirtyTimer = setTimeout(() => { dirtySince = null; }, 5000);
  }

  onDestroy(() => {
    if (pollHandle) clearInterval(pollHandle);
    if (dirtyTimer) clearTimeout(dirtyTimer);
    if (unlistenSSE) unlistenSSE();
  });

  onMount(async () => {
    await refreshChips();

    // Listen for SSE push notifications (realtime sync). When the office
    // server broadcasts a chip change, the backend emits
    // `condition-chips-changed` and we refresh immediately instead of waiting
    // for the 30s poll.
    try {
      unlistenSSE = await listen('condition-chips-changed', () => {
        refreshChips();
      });
      // Start the SSE subscription on the backend (long-lived task). Safe to
      // call when not paired — the command returns immediately in that case.
      await invoke('subscribe_condition_chips');
    } catch (e) {
      console.error('Failed to start chip sync subscription:', e);
    }
  });

  // Only poll when sync is enabled — avoids pointless DB reads for users
  // who haven't opted into chip sync (the default).
  $effect(() => {
    if (settings.state.sync_condition_chips) {
      pollHandle = setInterval(refreshChips, 30_000);
      return () => { if (pollHandle) clearInterval(pollHandle); };
    }
  });

  async function refreshChips() {
    try {
      const result = await listConditionChips();
      // Only update if the list actually changed (avoid unnecessary re-renders).
      if (
        result.length !== chips.length ||
        result.some((c, i) => c.id !== chips[i]?.id || c.sort_order !== chips[i]?.sort_order)
      ) {
        // If the user made a local change within the last 5s, the poll is
        // about to clobber it — surface a toast instead of silently
        // overwriting their edit.
        if (dirtySince !== null) {
          toasts.add({
            message: 'Condition chips updated from another machine',
            type: 'success',
            autoDismiss: true,
          });
        }
        chips = result;
      }
    } catch (e) {
      console.error('Failed to load condition chips:', e);
    }
    loaded = true;
  }

  async function addNewCondition() {
    const trimmed = newCondition.trim();
    if (!trimmed) return;
    // Dedup check (case-insensitive).
    if (displayChips.some((c) => c.text.toLowerCase() === trimmed.toLowerCase())) {
      newCondition = '';
      adding = false;
      return;
    }
    try {
      markDirty();
      chips = await addConditionChip(trimmed);
    } catch (e) {
      console.error('Failed to add condition chip:', e);
    }
    newCondition = '';
    adding = false;
  }

  async function removeCondition(conditionText: string) {
    try {
      markDirty();
      chips = await removeConditionChip(conditionText);
    } catch (e) {
      console.error('Failed to remove condition chip:', e);
    }
  }

  // Drag-and-drop using pointer events (not HTML5 DnD API, which is
  // unreliable in Tauri's webview — dragover/drop events don't fire
  // reliably). Pointer events work identically in all webviews.
  //
  // Flow: pointerdown marks a potential drag start. pointermove beyond a
  // small threshold activates the drag (capture pointer, start hit-testing).
  // pointerup performs the reorder if a drag was active, otherwise lets the
  // click pass through to the button.
  let pointerDownIndex = $state<number | null>(null);
  let dragIndex = $state<number | null>(null);
  let dragOverIndex = $state<number | null>(null);
  let isDragging = $state(false);
  let startX = 0;
  let startY = 0;

  let wasDragging = false; // set after a drag to suppress the subsequent click

  const DRAG_THRESHOLD = 5; // px — movement beyond this activates drag

  function handlePointerDown(e: PointerEvent, index: number) {
    if (e.button !== 0 || !loaded || chips.length === 0) return;
    pointerDownIndex = index;
    startX = e.clientX;
    startY = e.clientY;
  }

  function handlePointerMove(e: PointerEvent) {
    if (pointerDownIndex === null || isDragging) {
      if (isDragging) {
        // Hit-test: find which chip wrapper is under the pointer.
        const el = document
          .elementFromPoint(e.clientX, e.clientY)
          ?.closest('.condition-chip-wrapper') as HTMLElement | null;
        if (el) {
          const idx = Number(el.dataset.index);
          if (!Number.isNaN(idx)) {
            dragOverIndex = idx;
          }
        }
      }
      return;
    }
    // Check if movement exceeds threshold to activate drag.
    const dx = Math.abs(e.clientX - startX);
    const dy = Math.abs(e.clientY - startY);
    if (dx > DRAG_THRESHOLD || dy > DRAG_THRESHOLD) {
      isDragging = true;
      dragIndex = pointerDownIndex;
      // Capture pointer so we get pointermove/up even outside this element.
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    }
  }

  async function handlePointerUp(e: PointerEvent) {
    const didDrag = isDragging;
    const dropIndex = dragOverIndex;
    const srcIndex = dragIndex;

    if (didDrag) {
      (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
    }

    // Reset pointer state.
    pointerDownIndex = null;
    isDragging = false;
    dragIndex = null;
    dragOverIndex = null;

    if (didDrag) {
      // Set wasDragging so the subsequent click event on the button is
      // suppressed, then reset it on the next tick so future clicks work.
      wasDragging = true;
      setTimeout(() => { wasDragging = false; }, 0);

      if (dropIndex !== null && dropIndex !== srcIndex) {
        const reordered = [...chips];
        const [moved] = reordered.splice(srcIndex!, 1);
        reordered.splice(dropIndex, 0, moved);
        chips = reordered; // optimistic UI update

        const orderedIds = reordered.map((c) => c.id);
        markDirty();
        try {
          chips = await reorderConditionChips(orderedIds);
        } catch (err) {
          console.error('Failed to reorder condition chips:', err);
        }

        // First successful drag — dismiss the hint permanently.
        if (showDragHint) {
          showDragHint = false;
          try { localStorage.setItem('hasDraggedChips', '1'); } catch { /* localStorage may be unavailable */ }
        }
      }
    }
    // If not dragging, the click passes through to the button normally.
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      addNewCondition();
    } else if (e.key === 'Escape') {
      adding = false;
      newCondition = '';
    }
  }
</script>

<div class="condition-chips" role="group" aria-label="Common conditions quick-add">
  {#each displayChips as chip, i (chip.text)}
    <div
      class="condition-chip-wrapper"
      role="listitem"
      data-index={i}
      class:drag-over={dragOverIndex === i && dragIndex !== null}
      class:dragging={dragIndex === i}
      class:drag-active={isDragging}
      onpointerdown={(e) => handlePointerDown(e, i)}
      onpointermove={handlePointerMove}
      onpointerup={handlePointerUp}
      style:opacity={dragIndex === i ? '0.4' : '1'}
    >
      <span class="chip-grip" aria-hidden="true">⠿</span>
      <button
        class="condition-chip"
        type="button"
        onclick={(e) => { if (wasDragging) { e.preventDefault(); return; } onAdd(chip.text); }}
        title={`Add "${chip.text}" to the list`}
      >
        {chip.text}
      </button>
      <button
        class="chip-remove"
        type="button"
        onclick={() => removeCondition(chip.text)}
        title={`Remove "${chip.text}" from chips`}
        aria-label="Remove {chip.text}"
      >
        ×
      </button>
    </div>
  {/each}
  {#if adding}
    <input
      class="chip-input"
      type="text"
      bind:value={newCondition}
      onkeydown={handleKeydown}
      onblur={addNewCondition}
      placeholder="Condition name…"
      maxlength="60"
    />
  {:else}
    <button
      class="chip-add"
      type="button"
      onclick={() => { adding = true; newCondition = ''; }}
      title="Add a new condition chip"
    >
      +
    </button>
  {/if}
  {#if showDragHint && loaded}
    <p class="drag-hint">⠿ Drag to reorder · Click text to add</p>
  {/if}
</div>

<style>
  .condition-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    margin-bottom: 6px;
    /* Guarantee the chip row can wrap fully without being clipped by a
       flex parent that has a fixed/min width. */
    width: 100%;
    box-sizing: border-box;
  }

  /* The pill background lives on the wrapper so the chip always looks like a
     complete capsule — even when the remove button's × is hidden. Previously
     the background was split across two buttons and the invisible remove
     button left a flat-edged gap, making every chip look "cut off". */
  .condition-chip-wrapper {
    display: inline-flex;
    align-items: center;
    max-width: 100%;
    border-radius: 12px;
    background-color: color-mix(in srgb, var(--success, #22c55e) 10%, transparent);
    border: 1px solid color-mix(in srgb, var(--success, #22c55e) 30%, transparent);
    overflow: hidden;
    user-select: none;
    -webkit-user-select: none;
  }

  /* Grip handle — visual affordance for drag. Muted by default, brightens
     on hover so the user sees the chip is interactive beyond click. */
  .chip-grip {
    flex: 0 0 auto;
    padding: 4px 2px 4px 6px;
    font-size: 10px;
    line-height: 1.4;
    color: var(--success, #22c55e);
    opacity: 0.35;
    cursor: grab;
    user-select: none;
    transition: opacity 0.15s ease;
  }

  .condition-chip-wrapper:hover .chip-grip {
    opacity: 0.7;
  }

  .condition-chip-wrapper:hover {
    background-color: color-mix(in srgb, var(--success, #22c55e) 18%, transparent);
    border-color: color-mix(in srgb, var(--success, #22c55e) 45%, transparent);
  }

  .condition-chip-wrapper.drag-over {
    border-left: 2px solid var(--accent, #3b82f6);
  }

  /* Only show grab cursor when loaded (draggable is active) */
  .condition-chip-wrapper.drag-active {
    cursor: grabbing;
  }

  .condition-chip-wrapper:not(.dragging):hover {
    cursor: grab;
  }

  .condition-chip {
    /* Label fills the available width so long condition names wrap/truncate
       cleanly instead of pushing the × off the edge of the sidebar. */
    min-width: 0;
    padding: 4px 8px;
    font-size: 12px;
    font-weight: 500;
    line-height: 1.4;
    color: var(--success, #22c55e);
    background: none;
    border: none;
    cursor: pointer;
    transition: color 0.15s ease;
  }

  /* The × is the only thing that fades — the pill itself stays intact. It
    starts faintly visible so the chip reads as removable even without hover. */
  .chip-remove {
    flex: 0 0 auto;
    padding: 4px 7px 4px 4px;
    font-size: 13px;
    line-height: 1;
    color: var(--success, #22c55e);
    background: none;
    border: none;
    cursor: pointer;
    opacity: 0.45;
    transition: opacity 0.15s ease, color 0.15s ease;
  }

  .condition-chip-wrapper:hover .chip-remove {
    opacity: 1;
  }

  .chip-remove:hover {
    color: var(--danger, #ef4444);
  }

  .chip-add {
    padding: 3px 12px;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-muted);
    background: none;
    border: 1px dashed var(--border, #444);
    border-radius: 12px;
    cursor: pointer;
    transition: color 0.15s ease, border-color 0.15s ease;
  }

  .chip-add:hover {
    color: var(--success, #22c55e);
    border-color: var(--success, #22c55e);
  }

  .chip-input {
    padding: 4px 9px;
    font-size: 12px;
    color: var(--text-primary);
    background-color: var(--bg-primary, #1a1a1a);
    border: 1px solid var(--success, #22c55e);
    border-radius: 12px;
    width: 140px;
    box-sizing: border-box;
  }

  .chip-input:focus {
    outline: none;
  }

  /* Hint text shown until the user's first successful drag. Tiny and muted
     so it's unobtrusive but discoverable. Auto-dismisses via localStorage. */
  .drag-hint {
    width: 100%;
    margin: 2px 0 0 0;
    font-size: 9px;
    color: var(--text-muted, #666);
    opacity: 0.7;
    user-select: none;
  }
</style>
