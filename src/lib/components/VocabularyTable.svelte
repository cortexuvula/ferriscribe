<script lang="ts">
  import type { VocabularyEntry } from '../api/vocabulary';
  import { filterVocabularyEntries } from '../utils/vocabularyFilter';

  interface Props {
    entries: VocabularyEntry[];
    loading: boolean;
    searchText: string;
    categoryLabel: (value: string) => string;
    onEdit: (entry: VocabularyEntry) => void;
    onDelete: (entry: VocabularyEntry) => void;
    onToggleEnabled: (entry: VocabularyEntry) => void;
  }

  const { entries, loading, searchText, categoryLabel, onEdit, onDelete, onToggleEnabled }: Props = $props();

  const filtered = $derived(filterVocabularyEntries(entries, searchText));
</script>

<div class="vocab-table-wrap">
  {#if loading}
    <p class="loading-text">Loading...</p>
  {:else if filtered.length === 0}
    <p class="empty-text">No vocabulary entries found.</p>
  {:else}
    <table class="vocab-table">
      <thead>
        <tr>
          <th>Find</th>
          <th>Replace With</th>
          <th class="col-category">Category</th>
          <th class="col-enabled">Enabled</th>
          <th class="col-actions">Actions</th>
        </tr>
      </thead>
      <tbody>
        {#each filtered as entry (entry.id)}
          <tr class:disabled={!entry.enabled}>
            <td class="mono">{entry.find_text}</td>
            <td class="truncate">{entry.replacement}</td>
            <td class="col-category">{categoryLabel(entry.category)}</td>
            <td class="col-enabled">
              <input
                type="checkbox"
                checked={entry.enabled}
                onchange={() => onToggleEnabled(entry)}
              />
            </td>
            <td class="col-actions actions">
              <button class="btn-edit" onclick={() => onEdit(entry)}>Edit</button>
              <button class="btn-delete" onclick={() => onDelete(entry)}>Del</button>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>

<style>
  .vocab-table-wrap {
    padding: 8px 20px 16px;
  }
  .loading-text, .empty-text {
    text-align: center;
    color: var(--text-secondary, #888);
    padding: 32px;
    font-size: 0.9rem;
  }
  .vocab-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.88rem;
    table-layout: fixed;
  }
  .vocab-table th {
    text-align: left;
    padding: 8px 8px;
    border-bottom: 1px solid var(--border-color, #333);
    color: var(--text-secondary, #888);
    font-weight: 500;
    font-size: 0.8rem;
    text-transform: uppercase;
    letter-spacing: 0.03em;
    position: sticky;
    top: 0;
    background: var(--bg-secondary, #1e1e1e);
    z-index: 1;
  }
  .vocab-table td {
    padding: 8px;
    border-bottom: 1px solid var(--border-color, #222);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .vocab-table tr.disabled td { opacity: 0.45; }
  .vocab-table tr:hover td { background: rgba(255, 255, 255, 0.03); }
  .mono { font-family: 'SF Mono', Menlo, Consolas, monospace; color: var(--text-primary, #e0e0e0); }
  .truncate { max-width: 0; }
  .col-category { width: 140px; color: var(--text-secondary, #aaa); }
  .col-enabled { width: 70px; text-align: center; }
  .col-enabled input { cursor: pointer; }
  .col-actions { width: 110px; }
  .actions { display: flex; gap: 4px; }
  .btn-edit, .btn-delete {
    padding: 3px 10px;
    border-radius: 3px;
    border: 1px solid var(--border-color, #444);
    background: transparent;
    color: var(--text-secondary, #bbb);
    cursor: pointer;
    font-size: 0.78rem;
  }
  .btn-edit:hover { background: rgba(255, 255, 255, 0.05); }
  .btn-delete { color: #ff6b6b; border-color: #ff6b6b44; }
  .btn-delete:hover { background: rgba(255, 107, 107, 0.08); }

  /* Sizing override for the enabled-column checkbox — replaces the
     .vocab-dialog input[type="checkbox"] global override from the
     pre-split parent so this checkbox renders at 14px like before. */
  input[type="checkbox"] {
    width: 14px !important;
    height: 14px;
    min-width: 14px;
    padding: 0;
    margin: 0;
    vertical-align: middle;
  }
</style>
