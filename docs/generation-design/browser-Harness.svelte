<script lang="ts">
import GenerateTab from '../../src/lib/pages/GenerateTab.svelte';
import {audit} from './browser-tauri';
</script>
<div class="fixture-label">SYNTHETIC · real GenerateTab · OS/provider/output storage simulated · no provenance backend</div>
<GenerateTab onNavigateRecordings={()=>audit.navigation++}/>
<style>.fixture-label {flex:none;padding:6px 10px;font:11px system-ui;color:var(--text-primary);background:var(--bg-secondary);border-bottom:1px solid var(--border);}</style>
