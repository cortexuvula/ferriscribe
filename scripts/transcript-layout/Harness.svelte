<script lang="ts">
  import EditorTab from '../../src/lib/pages/EditorTab.svelte';
  import { recordings } from '../../src/lib/stores/recordings.svelte';
  import type { Recording } from '../../src/lib/types';

  // Each case navigates to a new document. These fixtures are initialization-only;
  // editing remains reactive through the real recording store and EditorTab.
  const params = new URLSearchParams(location.search);
  const outcome = params.get('outcome') || 'completed-with-unassigned';
  const blocked = params.has('bypass');
  const transcript = blocked
    ? 'Speaker 1: Synthetic assigned passage followed by synthetic suspect words.'
    : 'Synthetic unchanged plain passage.';
  const metadata: Record<string, unknown> = outcome === 'unknown' ? {} : { diarization_outcome: outcome };
  if (blocked) {
    if (params.get('evidence') !== 'unknown') metadata.diarization_fold_evidence = 'fold_possible';
    const version = params.get('version');
    if (version !== 'absent') metadata.transcript_format_version = Number(version);
  } else {
    metadata.transcript_segments = [{ speaker: null, text: transcript, start: 0, end: 1 }];
  }
  recordings.selectedRecording = {
    id: 'synthetic-gate-record', filename: 'synthetic-fixture.wav', transcript,
    soap_note: null, referral: null, letter: null, peer_discussion: null,
    chat: null, patient_name: null, audio_path: '', duration_seconds: null,
    file_size_bytes: null, stt_provider: null, ai_provider: null,
    metadata, status: { status: 'pending' }, tags: [], created_at: '',
  } satisfies Recording;
</script>

<div class="fixture">SYNTHETIC ONLY · layout regression · production EditorTab · no native bridge</div>
<main><EditorTab tabId="transcript" /></main>

<style>
  :global(body) { margin: 0; }
  :global(#app) { display: flex; flex-direction: column; height: 100vh; }
  .fixture { padding: 5px 12px; font-size: 11px; background: var(--bg-secondary); color: var(--text-secondary); }
  main { display: flex; flex: 1; min-height: 0; overflow: hidden; }
</style>
