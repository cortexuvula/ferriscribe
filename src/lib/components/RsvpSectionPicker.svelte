<script lang="ts">
  import { rsvp } from '../stores/rsvp';
  import { settings } from '../stores/settings';
  import { tokenize, type Section } from '../rsvp/engine';

  let selected = $state<Set<string>>(new Set());
  let rememberChoice = $state(false);
  let initialised = false;

  $effect(() => {
    if (!$rsvp.picker.open) {
      initialised = false;
      return;
    }
    if (initialised) return;
    initialised = true;

    rememberChoice = $settings.rsvp_remember_sections;
    const remembered = new Set($settings.rsvp_remembered_sections);
    const useRemembered =
      $settings.rsvp_remember_sections && remembered.size > 0;

    selected = new Set(
      $rsvp.picker.sections
        .filter((s) => (useRemembered ? remembered.has(s.name) : true))
        .map((s) => s.name),
    );
  });

  function toggle(name: string): void {
    const next = new Set(selected);
    if (next.has(name)) next.delete(name); else next.add(name);
    selected = next;
  }

  function start(): void {
    const picked = $rsvp.picker.sections.filter((s) => selected.has(s.name));
    if (picked.length === 0) return;
    const pieces = picked.map((s) => sectionText($rsvp.picker.text, s));
    const joined = pieces.join('\n\n');

    if (rememberChoice) {
      settings.updateField('rsvp_remember_sections', true);
      settings.updateField('rsvp_remembered_sections', [...selected]);
    } else if ($settings.rsvp_remember_sections) {
      settings.updateField('rsvp_remember_sections', false);
    }

    rsvp.startReading(joined, 'soap');
  }

  function sectionText(fullText: string, s: Section): string {
    // Reconstruct the section by word index from the original text.
    const tokens = tokenize(fullText);
    return tokens.slice(s.startWordIndex, s.endWordIndex).map((t) => t.word).join(' ');
  }

  function cancel(): void {
    rsvp.closeAll();
  }
</script>

{#if $rsvp.picker.open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div class="backdrop" onclick={cancel} role="presentation">
    <div class="dialog" role="dialog" aria-modal="true" tabindex="-1" onclick={(e) => e.stopPropagation()}>
      <div class="header">
        <h2>Choose sections to read</h2>
        <button class="btn-close" aria-label="Close" onclick={cancel}>&times;</button>
      </div>

      <ul class="sections">
        {#each $rsvp.picker.sections as section (section.name)}
          <li>
            <label>
              <input
                type="checkbox"
                checked={selected.has(section.name)}
                onchange={() => toggle(section.name)}
              />
              <span class="name">{section.name}</span>
              <span class="count">~{section.wordCount} words</span>
            </label>
          </li>
        {/each}
      </ul>

      <label class="remember">
        <input type="checkbox" bind:checked={rememberChoice} />
        Remember my selection
      </label>

      <div class="actions">
        <button class="btn-cancel" onclick={cancel}>Cancel</button>
        <button class="btn-start" onclick={start} disabled={selected.size === 0}>
          Start reading
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1500;
  }
  .dialog {
    background: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    padding: 20px 24px;
    width: min(480px, 90vw);
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    gap: 14px;
    box-shadow: var(--shadow-lg);
  }
  .header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .header h2 {
    margin: 0;
    font-size: 1.05rem;
  }
  .btn-close {
    background: none;
    border: none;
    color: var(--text-secondary);
    font-size: 1.4rem;
    line-height: 1;
    cursor: pointer;
    padding: 4px 8px;
    border-radius: 4px;
  }
  .btn-close:hover { background: var(--bg-hover); }

  /* Override the app's global `input { width: 100% }` so checkboxes don't
     stretch and push the label text outside the dialog. */
  .dialog input[type="checkbox"] {
    width: 14px !important;
    height: 14px;
    min-width: 14px;
    flex: 0 0 auto;
    margin: 0;
    padding: 0;
  }

  .sections {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .sections li + li { border-top: 1px solid var(--border); }
  .sections label {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    cursor: pointer;
  }
  .sections label:hover { background: var(--bg-hover); }
  .sections .name { flex: 1; font-weight: 500; }
  .sections .count { color: var(--text-secondary); font-size: 0.85rem; white-space: nowrap; }

  .remember {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 0.88rem;
    color: var(--text-secondary);
    cursor: pointer;
    white-space: nowrap;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .btn-cancel, .btn-start {
    padding: 8px 16px;
    border-radius: 6px;
    font-size: 0.9rem;
    cursor: pointer;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-primary);
  }
  .btn-start {
    background: var(--accent);
    color: white;
    border-color: var(--accent);
  }
  .btn-start:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .btn-start:not(:disabled):hover { filter: brightness(1.1); }
  .btn-cancel:hover { background: var(--bg-hover); }
</style>
