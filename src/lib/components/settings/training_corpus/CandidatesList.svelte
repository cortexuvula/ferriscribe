<script lang="ts">
  import ReviewLayout from './ReviewLayout.svelte';
  let { onchange }: { onchange: () => void } = $props();
</script>

<ReviewLayout mode="candidate" {onchange} />
