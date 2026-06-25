<script lang="ts">
  import ReviewLayout from './ReviewLayout.svelte';
  const { onchange }: { onchange: () => void } = $props();
</script>

<ReviewLayout mode="rejected" {onchange} />
