<script lang="ts">
  /**
   * Shared inline callout for the settings panes — the themed replacement
   * for the per-file amber/red warning boxes that had drifted into
   * hardcoded light-theme hexes. Colors derive from the semantic tokens
   * (--warning / --danger / --success / --info) so both themes render
   * correctly without per-usage overrides.
   */
  interface Props {
    kind: 'warning' | 'danger' | 'success' | 'info';
    /** ARIA role; defaults to 'alert' (announcement-worthy callouts). */
    role?: string;
    children?: import('svelte').Snippet;
  }

  let { kind, role = 'alert', children }: Props = $props();
</script>

<div class={'callout ' + kind} {role}>{@render children?.()}</div>

<style>
  .callout {
    border-radius: 4px;
    padding: 8px 12px;
    font-size: 0.85rem;
    line-height: 1.45;
    color: var(--text-primary);
  }

  .callout.warning {
    background: color-mix(in srgb, var(--warning) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--warning) 35%, transparent);
  }

  .callout.danger {
    background: color-mix(in srgb, var(--danger) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--danger) 35%, transparent);
  }

  .callout.success {
    background: color-mix(in srgb, var(--success) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--success) 35%, transparent);
  }

  .callout.info {
    background: color-mix(in srgb, var(--info) 12%, transparent);
    border: 1px solid color-mix(in srgb, var(--info) 35%, transparent);
  }
</style>
