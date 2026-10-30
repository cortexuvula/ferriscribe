<script lang="ts">
  import { onDestroy } from 'svelte';

  interface Props {
    value?: string;
    placeholder?: string;
    /// Accessible label + input id. Configurable because the Active and
    /// Trash views render a SearchBar each — a hardcoded id would collide.
    label?: string;
    inputId?: string;
    /// Drop the surrounding chrome (padding/border/background) so a parent
    /// filter bar can own the row's framing. Default keeps the standalone
    /// look the Trash panel uses.
    bare?: boolean;
    onSearch?: (query: string) => void;
  }

  let {
    value = $bindable(''),
    placeholder = 'Search…',
    label = 'Search recordings',
    inputId = 'search-input',
    bare = false,
    onSearch = () => {},
  }: Props = $props();

  let debounceTimer: ReturnType<typeof setTimeout> | null = null;
  /// The value this component last emitted LOCALLY (user typing). When the
  /// parent rewrites `value` to something else — a programmatic Clear
  /// search / Clear filters while a debounce is still pending — the stale
  /// callback must die with it: a pending onSearch firing 300ms later
  /// would resurrect the filter the user just cleared.
  let lastLocalValue = value;

  function handleInput(e: Event) {
    const input = e.target as HTMLInputElement;
    value = input.value;
    lastLocalValue = value;
    if (debounceTimer !== null) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => onSearch(value), 300);
  }

  function cancelPending() {
    if (debounceTimer !== null) {
      clearTimeout(debounceTimer);
      debounceTimer = null;
    }
  }

  // Programmatic value change (parent binding rewritten): adopt it and
  // cancel any pending debounce. Local typing sets `lastLocalValue` first,
  // so its own reactivity never trips this.
  $effect(() => {
    if (value !== lastLocalValue) {
      lastLocalValue = value;
      cancelPending();
    }
  });

  onDestroy(cancelPending);
</script>

<div class="search-bar" class:bare>
  <label for={inputId} class="sr-only">{label}</label>
  <input
    id={inputId}
    type="text"
    {value}
    {placeholder}
    aria-label={label}
    oninput={handleInput}
    class="search-input"
  />
</div>

<style>
  .search-bar {
    padding: 10px 12px;
    flex-shrink: 0;
    border-bottom: 1px solid var(--border);
    background-color: var(--bg-secondary);
  }

  .search-bar.bare {
    padding: 0;
    border-bottom: none;
    background-color: transparent;
    width: 100%;
  }

  .search-input {
    width: 100%;
    border-radius: var(--radius-md);
    background-color: var(--bg-input);
    font-size: 13px;
  }
</style>
