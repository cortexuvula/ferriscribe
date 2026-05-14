<script lang="ts">
  import ReviewLayout from './ReviewLayout.svelte';
  let { onchange }: { onchange: () => void } = $props();
</script>

<ReviewLayout mode="rejected" {onchange} />
