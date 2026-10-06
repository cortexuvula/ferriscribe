<script lang="ts">
  import { onDestroy } from 'svelte';

  interface Props {
    value?: string;
    placeholder?: string;
    /// Accessible label + input id. Configurable because the Active and
    /// Trash views render a SearchBar each — a hardcoded id would collide.
    label?: string;
    inputId?: string;
    onSearch?: (query: string) => void;
  }

  let {
    value = $bindable(''),
    placeholder = 'Search…',
    label = 'Search recordings',
    inputId = 'search-input',
    onSearch = () => {},
  }: Props = $props();

  let debounceTimer: ReturnType<typeof setTimeout> | null = null;

  function handleInput(e: Event) {
    const input = e.target as HTMLInputElement;
    value = input.value;
    if (debounceTimer !== null) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(() => onSearch(value), 300);
  }

  onDestroy(() => {
    if (debounceTimer !== null) clearTimeout(debounceTimer);
  });
</script>

<div class="search-bar">
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

  .search-input {
    width: 100%;
    border-radius: var(--radius-md);
    background-color: var(--bg-input);
    font-size: 13px;
  }
</style>
