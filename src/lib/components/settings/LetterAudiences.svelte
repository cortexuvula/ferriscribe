<script lang="ts">
  import { onMount } from 'svelte';
  import { letterAudiences } from '../../stores/letterAudiences.svelte';
  import type { LetterAudience } from '../../types/letterAudience';

  type EditingAudience = {
    id: string;
    name: string;
    system_prompt: string;
    user_template: string;
  } | null;

  let editing = $state<EditingAudience>(null);
  let error = $state<string | null>(null);
  let saving = $state(false);

  onMount(() => {
    letterAudiences.list();
  });

  function startAdd() {
    editing = {
      id: '',
      name: '',
      system_prompt: '',
      user_template: '',
    };
  }

  function startEdit(audience: LetterAudience) {
    if (audience.is_builtin) return;
    editing = {
      id: audience.id,
      name: audience.name,
      system_prompt: audience.system_prompt,
      user_template: audience.user_template ?? '',
    };
  }

  function cancelEdit() {
    editing = null;
    error = null;
  }

  async function handleSave() {
    if (!editing) return;
    if (!editing.name.trim()) {
      error = 'Name is required.';
      return;
    }
    if (!editing.system_prompt.trim()) {
      error = 'System prompt is required.';
      return;
    }
    saving = true;
    error = null;
    try {
      const audience: LetterAudience = {
        id: editing.id || crypto.randomUUID(),
        name: editing.name.trim(),
        system_prompt: editing.system_prompt,
        user_template: editing.user_template.trim() || null,
        is_builtin: false,
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
      };
      await letterAudiences.upsert(audience);
      editing = null;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to save audience';
    } finally {
      saving = false;
    }
  }

  async function handleDelete(audience: LetterAudience) {
    if (audience.is_builtin) return;
    if (!confirm(`Delete audience "${audience.name}"? This cannot be undone.`)) return;
    try {
      await letterAudiences.delete(audience.id);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to delete audience';
    }
  }

  function viewPrompt(audience: LetterAudience) {
    alert(audience.system_prompt);
  }
</script>

<section class="settings-section">
  <header class="la-header">
    <h3 class="section-title">Letter Audiences</h3>
    <p class="section-desc">
      Configure audiences that determine how letters are addressed and formatted.
      Built-in audiences show their prompts; custom audiences can be edited or deleted.
    </p>
  </header>

  {#if letterAudiences.loading}
    <div class="la-loading">Loading audiences...</div>
  {:else}
    {#if editing}
      <div class="la-form">
        <h4 class="la-form-title">{editing.id ? 'Edit Audience' : 'Add Custom Audience'}</h4>

        {#if error}
          <div class="la-errors">{error}</div>
        {/if}

        <div class="form-group">
          <label for="audience-name" class="form-label">Name</label>
          <input
            id="audience-name"
            type="text"
            bind:value={editing.name}
            placeholder="e.g. School Nurse, Insurance Company"
          />
        </div>

        <div class="form-group">
          <label for="audience-system-prompt" class="form-label">System Prompt</label>
          <textarea
            id="audience-system-prompt"
            bind:value={editing.system_prompt}
            rows="12"
            placeholder="Instructions for how the AI generates letters for this audience..."
          ></textarea>
        </div>

        <div class="form-group">
          <label for="audience-user-template" class="form-label">User Template <span class="optional">(optional)</span></label>
          <textarea
            id="audience-user-template"
            bind:value={editing.user_template}
            rows="6"
            placeholder="Optional user-facing template wrapping the generated letter..."
          ></textarea>
        </div>

        <details class="la-placeholders">
          <summary>Available placeholders</summary>
          <ul>
            <li><code>{'{'}letter_type}</code> — Type of letter being generated (e.g. results, instructions, follow-up)</li>
            <li><code>{'{'}soap_note}</code> — The full SOAP note text from the consultation</li>
          </ul>
        </details>

        <div class="la-form-actions">
          <button
            class="btn btn-primary"
            onclick={handleSave}
            disabled={saving}
          >
            {saving ? 'Saving...' : 'Save'}
          </button>
          <button class="btn" onclick={cancelEdit}>
            Cancel
          </button>
        </div>
      </div>
    {/if}

    {#if error && !editing}
      <div class="la-errors">{error}</div>
    {/if}

    <div class="la-list">
      {#each letterAudiences.audiences as audience (audience.id)}
        <div class="la-item">
          <span class="la-item-name">{audience.name}</span>
          <div class="la-item-actions">
            {#if audience.is_builtin}
              <span class="la-badge">Built-in</span>
              <button class="btn btn-sm" onclick={() => viewPrompt(audience)}>
                View Prompt
              </button>
            {:else}
              <button class="btn btn-sm" onclick={() => startEdit(audience)}>
                Edit
              </button>
              <button class="btn btn-sm btn-danger" onclick={() => handleDelete(audience)}>
                Delete
              </button>
            {/if}
          </div>
        </div>
      {:else}
        <p class="la-empty">No audiences found.</p>
      {/each}
    </div>

    {#if !editing}
      <button class="btn btn-primary la-add-btn" onclick={startAdd}>
        Add Custom Audience
      </button>
    {/if}
  {/if}
</section>

<style>
  .settings-section {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .section-title {
    font-size: 15px;
    font-weight: 600;
    color: var(--text-primary);
    padding-bottom: 8px;
    border-bottom: 1px solid var(--border-light);
    margin-bottom: 4px;
  }

  .section-desc {
    font-size: 12px;
    color: var(--text-muted);
    margin-top: -8px;
  }

  .la-loading {
    padding: 2rem;
    text-align: center;
    color: var(--text-secondary);
  }

  .la-errors {
    background: #fee;
    border: 1px solid #fbb;
    padding: 0.5rem;
    border-radius: 4px;
    font-size: 13px;
  }

  .la-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .la-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 14px;
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .la-item-name {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-primary);
  }

  .la-item-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .la-badge {
    font-size: 11px;
    font-weight: 500;
    padding: 2px 8px;
    border-radius: 4px;
    background: var(--bg-tertiary, #374151);
    color: var(--text-muted);
    border: 1px solid var(--border);
  }

  .btn {
    padding: 6px 14px;
    font-size: 12px;
    font-weight: 500;
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background-color 0.15s ease;
    border: 1px solid var(--border);
    background: var(--bg-tertiary, #374151);
    color: var(--text-secondary);
  }

  .btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }

  .btn-sm {
    padding: 4px 10px;
    font-size: 11px;
  }

  .btn-primary {
    background-color: var(--accent);
    color: var(--text-inverse);
    border-color: var(--accent);
  }

  .btn-primary:hover {
    background-color: var(--accent-hover);
  }

  .btn-primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn-danger {
    color: #dc2626;
    border-color: #fca5a5;
  }

  .btn-danger:hover {
    background: #fef2f2;
  }

  .la-add-btn {
    align-self: flex-start;
    margin-top: 4px;
  }

  /* Edit / Add form */
  .la-form {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 16px;
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  .la-form-title {
    margin: 0;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .form-group {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .form-label {
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
  }

  .optional {
    font-weight: 400;
    color: var(--text-muted);
  }

  input[type='text'],
  textarea {
    width: 100%;
    font-family: var(--font-mono, monospace);
    font-size: 0.85rem;
    line-height: 1.4;
    padding: 0.5rem 0.75rem;
    background: var(--bg-input);
    color: var(--text-primary);
    border: 1px solid var(--border);
    border-radius: 6px;
    resize: vertical;
  }

  input[type='text'] {
    font-family: inherit;
  }

  textarea {
    min-height: 80px;
  }

  .la-form-actions {
    display: flex;
    gap: 8px;
  }

  .la-placeholders {
    background: var(--bg-card);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.5rem 0.75rem;
  }

  .la-placeholders summary {
    cursor: pointer;
    font-weight: 500;
    font-size: 13px;
  }

  .la-placeholders ul {
    margin: 0.5rem 0 0;
    padding-left: 1.25rem;
  }

  .la-placeholders li {
    font-size: 12px;
    color: var(--text-secondary);
    margin-bottom: 4px;
  }

  .la-placeholders code {
    background: var(--bg-code);
    padding: 0.1rem 0.3rem;
    border-radius: 3px;
    font-size: 0.85rem;
  }

  .la-empty {
    text-align: center;
    color: var(--text-muted);
    font-size: 13px;
    padding: 2rem;
  }
</style>
