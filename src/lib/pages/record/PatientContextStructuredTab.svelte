<script lang="ts">
  type Props = {
    medicationsText: string;
    allergiesText: string;
    conditionsText: string;
  };
  let {
    medicationsText = $bindable(''),
    allergiesText = $bindable(''),
    conditionsText = $bindable(''),
  }: Props = $props();
</script>

<div class="structured-fields">
  <label class="field-label" for="rt-medications">Medications (one per line)</label>
  <textarea
    id="rt-medications"
    class="context-textarea structured"
    placeholder="Lisinopril 10mg PO daily"
    bind:value={medicationsText}
    rows="3"
  ></textarea>

  <label class="field-label" for="rt-allergies">Allergies (one per line)</label>
  <textarea
    id="rt-allergies"
    class="context-textarea structured"
    placeholder="Penicillin (rash)"
    bind:value={allergiesText}
    rows="2"
  ></textarea>

  <label class="field-label" for="rt-conditions">Known conditions (one per line)</label>
  <textarea
    id="rt-conditions"
    class="context-textarea structured"
    placeholder="Type 2 diabetes"
    bind:value={conditionsText}
    rows="3"
  ></textarea>
</div>

<style>
  .structured-fields {
    display: flex;
    flex-direction: column;
    padding: 8px 12px 12px;
  }

  .field-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    margin-top: 8px;
    margin-bottom: 4px;
    display: block;
  }

  .context-textarea {
    display: block;
    width: 100%;
    padding: 8px 10px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--text-primary);
    background-color: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: 4px;
    resize: vertical;
    min-height: 80px;
    max-height: 200px;
  }

  .context-textarea.structured {
    min-height: 56px;
  }

  .context-textarea:focus {
    outline: none;
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .context-textarea::placeholder {
    color: var(--text-muted);
  }
</style>
