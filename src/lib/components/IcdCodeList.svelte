<script lang="ts">
  /**
   * Vertical billing-code list — one row per code, pairing an IcdChip
   * (amber when the code is not on the BC MSP list) with the code's
   * explaining title. The title comes from the note's own
   * "ICD-9 Code: <code> — <description>" line when present, otherwise
   * from the official MSP description (see `extractIcdCodesValidated`);
   * rows with neither render the chip alone. Replaces the former inline
   * chip row, which showed raw "ICD-9 Code: …" strings with no titles.
   */
  import type { ValidatedIcdCode } from '../icd';
  import IcdChip from './IcdChip.svelte';

  type Props = {
    codes: ValidatedIcdCode[];
    /** List heading, e.g. "Billing codes (ICD-9)". */
    label: string;
  };
  let { codes, label }: Props = $props();
</script>

<div class="icd-list" role="list" aria-label={label}>
  <span class="icd-list-label">{label}</span>
  {#each codes as code (code.raw)}
    <div class="icd-row" role="listitem">
      <IcdChip code={code.bare} valid={code.valid} />
      {#if code.description}
        <span class="icd-desc">{code.description}</span>
      {/if}
    </div>
  {/each}
</div>

<style>
  .icd-list {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
    text-align: left;
  }

  .icd-list-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .icd-row {
    display: flex;
    align-items: baseline;
    gap: 8px;
    flex-wrap: wrap;
  }

  .icd-desc {
    font-size: 12px;
    line-height: 1.4;
    color: var(--text-secondary);
  }
</style>
