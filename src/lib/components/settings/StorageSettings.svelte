<script lang="ts">
  import Backup from './Backup.svelte';
  import RecordingStorage from './sections/RecordingStorage.svelte';
  import RecordingRetention from './sections/RecordingRetention.svelte';
  import DatabaseSecurity from './sections/DatabaseSecurity.svelte';
</script>

<section class="settings-section settings-composition">
  <h3 class="section-title">Storage &amp; Backup</h3>
  <RecordingStorage />
  <DatabaseSecurity />
  <RecordingRetention />
  <Backup />
</section>
