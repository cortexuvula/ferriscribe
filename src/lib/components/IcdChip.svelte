<script lang="ts">
  /**
   * A single ICD code chip. Recolors to amber with a tooltip when the
   * code is not on the BC MSP ICD-9 list (the warning variant mirrors
   * EndpointHealthPill's `.warn` recipe). `valid === null` renders as
   * the neutral accent style with an explanatory tooltip — covers the
   * ICD-10-code-in-both-mode case ("validation unavailable") and the
   * store-load-failed case.
   */
  type Props = {
    code: string;
    valid: boolean | null;
    /** Tooltip shown when valid === null (e.g. ICD-10 code, or set not loaded). */
    neutralTooltip?: string;
  };
  let { code, valid, neutralTooltip = 'Validation unavailable' }: Props = $props();

  const tooltip = $derived(
    valid === false
      ? 'Not in BC MSP ICD-9 list — verify before billing'
      : valid === null
        ? neutralTooltip
        : '',
  );
</script>

<span class="icd-code" class:invalid={valid === false} title={tooltip}>
  {code}
</span>

<style>
  .icd-code {
    font-family: var(--font-mono, ui-monospace, monospace);
    color: var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 30%, transparent);
    border-radius: var(--radius-sm, 4px);
    padding: 1px 6px;
    font-size: 0.85em;
    white-space: nowrap;
  }
  .icd-code.invalid {
    color: var(--warning);
    background: color-mix(in srgb, var(--warning) 15%, transparent);
    border: 1px solid color-mix(in srgb, var(--warning) 35%, transparent);
  }
</style>
