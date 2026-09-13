<script>
import TranscriptView from './TranscriptView.svelte';
let value=$state(''); let segments=$state(undefined); let outcome=$state('unknown'); let serial=$state(0);
window.review={set: (v,s,o,reset=true)=>{value=v;segments=s;outcome=o;if(reset)serial++;}, get:()=>({value,segments,outcome}), changes:[]};
</script>
<div style="height:100vh;display:flex;flex-direction:column;background:var(--bg-primary)">
{#key serial}<TranscriptView {value} {segments} diarizationOutcome={outcome} onChange={(v)=>{window.review.changes.push(v);value=v;}} />{/key}
</div>