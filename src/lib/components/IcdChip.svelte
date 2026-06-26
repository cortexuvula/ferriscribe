<script lang="ts">
  /**
   * A single ICD code chip. Recolors to amber with a tooltip when the
   * code is not on the BC MSP ICD-9 list (the warning variant mirrors
   * EndpointHealthPill's `.warn` recipe). `valid === null` (set not
   * loaded, or an ICD-10 code in "both" mode) renders as the neutral
   * accent style — never a false warning.
   */
  type Props = {
    code: string;
    valid: boolean | null;
  };
  let { code, valid }: Props = $props();

  const tooltip = $derived(
    valid === false
      ? 'Not in BC MSP ICD-9 list — verify before billing'
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
