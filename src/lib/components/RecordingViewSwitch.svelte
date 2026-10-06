<script lang="ts">
  /// Accessible Active | Trash view switch (real tabs: roving tabindex +
  /// Arrow keys, aria-selected, aria-controls wiring). Visible even when
  /// the Active list is empty — an empty state must never hide the route
  /// to recovery.
  interface Props {
    view: 'active' | 'trash';
    /// Authoritative trashed count for the Trash(N) badge (0 renders no
    /// parenthetical).
    trashCount: number;
    onChange: (view: 'active' | 'trash') => void;
  }

  const { view, trashCount, onChange }: Props = $props();

  function focusTab(id: 'active' | 'trash') {
    requestAnimationFrame(() => {
      document.getElementById(`view-tab-${id}`)?.focus();
    });
  }

  /// Arrow-key navigation lives on each tab (the roving tabindex means the
  /// focused tab receives keys) — Left/Right move between the two views.
  function handleTabKeydown(e: KeyboardEvent) {
    if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
    e.preventDefault();
    const next: 'active' | 'trash' = view === 'active' ? 'trash' : 'active';
    onChange(next);
    focusTab(next);
  }
</script>

<div class="view-switch" role="tablist" aria-label="Recordings views">
  <button
    role="tab"
    id="view-tab-active"
    aria-selected={view === 'active'}
    aria-controls="view-panel-active"
    tabindex={view === 'active' ? 0 : -1}
    class:active={view === 'active'}
    onclick={() => onChange('active')}
    onkeydown={handleTabKeydown}
  >
    Active
  </button>
  <button
    role="tab"
    id="view-tab-trash"
    aria-selected={view === 'trash'}
    aria-controls="view-panel-trash"
    tabindex={view === 'trash' ? 0 : -1}
    class:active={view === 'trash'}
    class:has-items={trashCount > 0}
    onclick={() => onChange('trash')}
    onkeydown={handleTabKeydown}
  >
    Trash{trashCount > 0 ? ` (${trashCount})` : ''}
  </button>
</div>

<style>
  .view-switch {
    display: flex;
    gap: 4px;
    padding: 8px 12px 0;
    border-bottom: 1px solid var(--border);
  }

  button {
    padding: 8px 14px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-muted);
    background-color: transparent;
    border: 1px solid transparent;
    border-bottom: none;
    border-radius: var(--radius-sm) var(--radius-sm) 0 0;
    cursor: pointer;
    transition: color 0.15s ease, background-color 0.15s ease;
  }

  button:hover {
    color: var(--text-primary);
    background-color: var(--bg-hover);
  }

  button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }

  button.active {
    color: var(--text-primary);
    background-color: var(--bg-primary);
    border-color: var(--border);
    /* Cover the switch's bottom border so the active tab reads as open. */
    position: relative;
    top: 1px;
  }

  .has-items {
    color: var(--text-secondary);
  }
</style>
