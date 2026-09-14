<script lang="ts">
  // Note: deliberately NOT SvelteMap — getSpeakerColor assigns palette
  // entries lazily during template rendering, and mutating reactive state
  // inside a template expression is a Svelte 5 runtime error
  // (state_unsafe_mutation). The assignment is deterministic per label and
  // never needs to trigger reactivity, so a plain Map is correct.
  interface SpeakerSection {
    speaker: string | null;
    text: string;
  }

  // Recording-level diarization outcome from the backend pipeline.
  // - 'off': diarization was not requested (labelling disabled).
  // - 'skipped': diarization was requested but did not run (e.g. the
  //   diarization models were unavailable at transcription time).
  // - 'failed': diarization was attempted but errored (inference crash).
  // - 'completed': diarization ran and all segments have speaker labels.
  // - 'completed-with-unassigned': diarization ran but some segments have null speakers.
  // - 'unknown': metadata does not carry an outcome yet (frontend-only
  //   fallback — the outcome must come from persisted metadata, never be
  //   re-derived from structural side-effects).
  // The frontend MUST receive this explicitly — inferring state from
  // speaker:null or section count causes silent fallthrough to plain text.
  export type DiarizationOutcome =
    | 'off'
    | 'skipped'
    | 'failed'
    | 'completed'
    | 'completed-with-unassigned'
    | 'unknown';

  interface Props {
    value?: string;
    /** Structured segment data from recording metadata — preferred over text parsing. */
    segments?: Array<{ speaker: string | null; text: string; start: number; end: number }>;
    /** Recording-level diarization outcome. Defaults to 'unknown' (metadata
     *  not yet persisted).
     *  'off' = we KNOW labelling was not requested.
     *  'skipped' = we KNOW labelling was requested but did not run.
     *  'failed' = we KNOW diarization errored.
     *  'completed' / 'completed-with-unassigned' = real persisted metadata.
     *  'unknown' = metadata missing; show explicit unavailability message. */
    diarizationOutcome?: DiarizationOutcome;
    /** Persisted bounded reason code for the 'skipped' outcome, transported
     *  verbatim from recording metadata (EditorTab validates it). Only
     *  'models_unavailable' (REASON_MODELS_UNAVAILABLE in
     *  src-tauri/src/commands/transcription/inner.rs) currently carries
     *  confirming wording. Missing/unrecognised → generic wording; the view
     *  must never invent a cause. Keyed off the SAVED code, never off
     *  current settings or installed models. */
    skipReason?: 'models_unavailable';
    /** Persisted bounded reason code for the 'failed' outcome, transported
     *  verbatim from recording metadata (EditorTab validates it). Only
     *  'provider_error' (REASON_PROVIDER_ERROR in
     *  src-tauri/src/commands/transcription/inner.rs) currently carries
     *  confirming wording. Missing/unrecognised → generic wording; the view
     *  must never invent a cause or surface raw provider error text. */
    failReason?: 'provider_error';
    /** Affordance for the 'skipped' outcome: opens Audio settings. The parent
     *  wires this to the shared settings navigation. Deliberately NOT an
     *  automatic retry — the user decides whether to re-transcribe. */
    onOpenAudioSettings?: () => void;
    placeholder?: string;
    onChange?: (v: string) => void;
    /** Fires on every keystroke while editing (not debounced, not save-triggered).
     *  The parent uses this to track the current draft for Copy without waiting
     *  for doneEdit/save. Does NOT replace onChange — that still fires only on
     *  Done (save trigger). */
    onDraftChange?: (v: string) => void;
  }

  const { value = '', segments, diarizationOutcome = 'unknown', skipReason, failReason, onOpenAudioSettings, placeholder = '', onChange = () => {}, onDraftChange = () => {} }: Props = $props();

  let editing = $state(false);
  // svelte-ignore state_referenced_locally
  // editText is initialized from the prop and re-synced in startEdit()
  // before the editor is shown, so it always reflects the latest external
  // value when editing begins.
  let editText = $state(value);

  // Debounce the transcript parse so typing into the editor doesn't
  // re-parse the entire transcript on every keystroke. The parent
  // (EditorTab) optimistically updates `value` per keystroke; without
  // this debounce, a long transcript would jank the editor.
  //
  // We use a non-reactive cache (`parseCache`) updated by a timer, NOT
  // `$state` — writing `$state` inside a `$effect` that feeds a `$derived`
  // is a Svelte 5 anti-pattern (effect-writes-state-it-reads). Instead, a
  // `$state` version counter (`parseVersion`) bumps when the timer fires,
  // and `$derived.by` reads both `value` (tracked) and `parseCache`
  // (untracked) gated on `parseVersion` (tracked) to re-run only when the
  // debounce window elapses.
  const DEBOUNCE_MS = 400;
  let parseTimer: ReturnType<typeof setTimeout> | null = null;
  // svelte-ignore state_referenced_locally
  let parseCache = value; // non-reactive — read inside $derived but not tracked
  let parseVersion = $state(0);

  $effect(() => {
    // Track `value` so the effect re-runs when it changes.
    const current = value;
    if (parseTimer !== null) clearTimeout(parseTimer);
    parseTimer = setTimeout(() => {
      parseCache = current;
      parseVersion++; // bump triggers the $derived to re-parse
    }, DEBOUNCE_MS);
    // Cleanup on re-run or unmount: clear the pending timer.
    return () => {
      if (parseTimer !== null) clearTimeout(parseTimer);
    };
  });

  // Parse the transcript into speaker sections.
  // Uses structured segments from metadata when available (more reliable),
  // falls back to regex parsing of "Speaker N: text" formatted text.
  // Reads `parseVersion` (tracked — re-runs on debounce tick) and
  // `parseCache` (untracked — the debounced value).
  const sections: SpeakerSection[] = $derived.by(() => {
    // Read parseVersion so this re-runs when the debounce fires.
    // The conditional keeps this a valid statement (not a bare expression).
    if (parseVersion < 0) return [];
    if (segments && segments.length > 0) {
      return groupSegmentsIntoSections(segments);
    }
    return parseTextSections(parseCache);
  });
  const hasSpeakers = $derived(sections.some((s) => s.speaker !== null));
  // Recording-level diarization outcome: did the backend attempt labelling?
  // 'completed', 'completed-with-unassigned', or 'failed' → diarization ran
  // (or tried); show structured view even when every segment is speaker:null.
  // 'skipped' and 'off' → labelling did not run: plain transcript, with the
  // skipped banner (below) carrying the status for 'skipped'. Rendering
  // skipped segments as "Speaker unassigned" would assert an attribution
  // attempt that never happened. 'unknown' → metadata missing, render
  // explicit unavailability message (never infer from segment presence).
  const hasDiarizationResult = $derived(
    diarizationOutcome !== 'off' && diarizationOutcome !== 'unknown' && diarizationOutcome !== 'skipped',
  );
  // #108 finding (a) / contract line E: paragraph count is NOT evidence that
  // attribution was attempted. The structured view (badges, "Speaker
  // unassigned" headings, caveat) renders ONLY when there is a real
  // attribution signal: an explicit diarization outcome that ran, or actual
  // speaker labels in the data. A multi-paragraph hand-written note must
  // render as paragraphs, never as fabricated "attempted attribution".
  const useStructuredView = $derived(hasSpeakers || hasDiarizationResult);
  // Honest 'unknown' rendering: when outcome is 'unknown' AND the text has
  // no explicit speaker labels, show "Speaker-labelling status unavailable"
  // instead of silent plain text. Multi-paragraph text without labels is
  // still 'unknown' — paragraph count no longer promotes it.
  const showUnknownStatus = $derived(
    diarizationOutcome === 'unknown' && !hasSpeakers,
  );
  // 'skipped' rendering: requested but did not run. Distinct from 'off'
  // (not requested), 'failed' (attempted and errored), and 'unknown'
  // (metadata absent). The supporting line confirms a cause ONLY when the
  // persisted reason code says so — never from current settings/models.
  const showSkippedStatus = $derived(diarizationOutcome === 'skipped');
  // 'failed' rendering: attempted and errored — the PERSISTED failure must
  // stay distinguishable from completed-with-unassigned after reload
  // (review contract line F). Without this banner, an identical all-null
  // body renders identically for 'failed' and
  // 'completed-with-unassigned', inviting a wrong judgment of model
  // performance. Symmetric to the skipped banner: heading + reason from
  // the PERSISTED bounded reason code only — never current settings, never
  // an invented cause.
  const showFailedStatus = $derived(diarizationOutcome === 'failed');
  const PROVIDER_ERROR_REASON = 'provider_error';
  const failedReasonLine = $derived.by(() => {
    if (failReason === PROVIDER_ERROR_REASON) {
      return 'Speaker labelling errored during this transcription';
    }
    return 'Speaker labelling failed for this recording';
  });
  // Wording keyed off the SAVED bounded reason code only. Anything missing
  // or unrecognised → generic wording; we do not invent a cause.
  const MODELS_UNAVAILABLE_REASON = 'models_unavailable';
  const skippedReasonLine = $derived.by(() => {
    if (skipReason === MODELS_UNAVAILABLE_REASON) {
      return 'Required models weren\'t available for this transcription';
    }
    return 'Speaker labelling was not run for this recording';
  });

  function groupSegmentsIntoSections(
    segs: Array<{ speaker: string | null; text: string }>,
  ): SpeakerSection[] {
    const result: SpeakerSection[] = [];
    let currentSpeaker: string | null = null;
    let currentText = '';

    for (const seg of segs) {
      const label = seg.speaker;
      if (label !== currentSpeaker) {
        if (currentText) {
          result.push({ speaker: currentSpeaker, text: currentText.trim() });
        }
        currentSpeaker = label;
        currentText = seg.text.trim();
      } else {
        currentText += ' ' + seg.text.trim();
      }
    }
    if (currentText) {
      result.push({ speaker: currentSpeaker, text: currentText.trim() });
    }
    return result;
  }

  function parseTextSections(text: string): SpeakerSection[] {
    if (!text) return [];
    // Split on double-newline (paragraph breaks from format_transcript_with_speakers).
    const paragraphs = text.split(/\n\n+/);
    const result: SpeakerSection[] = [];

    for (const para of paragraphs) {
      // B3: stored/copied text is `HH:MM:SS,mmm --> HH:MM:SS,mmm [Speaker N]\ntext`
      // (format_transcript_with_speakers, 1-based labels). The optional
      // timestamp prefix is skipped so the badge shows just "Speaker N".
      const match = para.match(
        /^(?:\d{2}:\d{2}:\d{2},\d{3}\s*-->\s*\d{2}:\d{2}:\d{2},\d{3}\s*)?\[(Speaker \d+)\]\s*\n?([\s\S]*)$/,
      );
      // UNASSIGNED MARKER: `[Speaker unassigned]` (written by
      // format_transcript_with_speakers via
      // medical_processing::transcript_markers::SPEAKER_UNASSIGNED_MARKER —
      // same bytes, single source). An unattributable span must render as
      // its own unassigned section, never inherit the previous speaker.
      // The marker cannot be mis-parsed as a real speaker: it matches
      // neither this bracket arm (digits only) nor the colon arm below.
      const unassigned = para.match(
        /^(?:\d{2}:\d{2}:\d{2},\d{3}\s*-->\s*\d{2}:\d{2}:\d{2},\d{3}\s*)?\[Speaker unassigned\]\s*\n?([\s\S]*)$/,
      );
      // LEGACY colon form: `Speaker N: text` paragraphs (the pre-bracket
      // convention AND hand-edited text). Both formats stay supported —
      // the bracketed form must supplement, never replace, this parser
      // (compatibility regression flagged in review: replacing it stripped
      // badges from every stored colon-formatted transcript).
      const colon = para.match(/^(Speaker \d+):\s*([\s\S]*)$/);
      if (match) {
        result.push({ speaker: match[1], text: match[2] });
      } else if (unassigned) {
        result.push({ speaker: null, text: unassigned[1] });
      } else if (colon) {
        result.push({ speaker: colon[1], text: colon[2] });
      } else if (para.trim()) {
        // No speaker label — could be unlabeled text before first speaker
        // or text that was edited to remove labels.
        result.push({ speaker: null, text: para.trim() });
      }
    }
    return result;
  }

  // Deterministic color per speaker — hash the speaker label to a hue.
  // Plain Map is deliberate: mutating this cache during template rendering
  // is exactly what we want, and SvelteMap would throw
  // state_unsafe_mutation (see the note at the top of this file).
  const speakerColors = new Map<string, number>(); // eslint-disable-line svelte/prefer-svelte-reactivity
  // #108 finding (c) / contract line H: the old palette set badge TEXT to
  // the saturated base colour on a 12% tint — 1.82–3.42:1 in light mode,
  // four entries below 4.5:1 in dark. The palette now supplies ONLY the
  // border (colour-as-adjunct) and the tint background; the TEXT colour is
  // a theme-aware per-slot class (.slot-N below) computed to exceed 4.5:1
  // against the composited tint on both production surfaces in BOTH themes
  // (worst case ≥5:1 — see TranscriptView.contrast.test.ts, which
  // recomputes the ratios from this file's own CSS rather than trusting a
  // parallel constant table).
  const palette = [
    { bg: 'rgba(59, 130, 246, 0.12)', border: '#3b82f6' },   // blue
    { bg: 'rgba(16, 185, 129, 0.12)', border: '#10b981' },   // emerald
    { bg: 'rgba(168, 85, 247, 0.12)', border: '#a855f7' },   // purple
    { bg: 'rgba(245, 158, 11, 0.12)', border: '#f59e0b' },   // amber
    { bg: 'rgba(236, 72, 153, 0.12)', border: '#ec4899' },   // pink
    { bg: 'rgba(6, 182, 212, 0.12)', border: '#06b6d4' },    // cyan
    { bg: 'rgba(132, 204, 22, 0.12)', border: '#84cc16' },   // lime
    { bg: 'rgba(244, 63, 94, 0.12)', border: '#f43f5e' },    // rose
  ];

  function getSpeakerSlot(speaker: string): number {
    let idx = speakerColors.get(speaker);
    if (idx === undefined) {
      idx = speakerColors.size % palette.length;
      speakerColors.set(speaker, idx);
    }
    return idx;
  }

  function startEdit() {
    editText = value;
    editing = true;
  }

  function doneEdit() {
    onChange(editText);
    editing = false;
  }

  // Notify the parent of every keystroke while editing so Copy can export
  // the current draft without waiting for Done/save. The effect only fires
  // when `editText` changes AND we are in editing mode — programmatic value
  // syncs (startEdit resetting editText) happen before `editing` is set, so
  // they don't trigger a spurious notification.
  $effect(() => {
    if (!editing) return;
    onDraftChange(editText);
  });
</script>

<div class="transcript-view">
  <!-- Uncertainty-honesty caveat: persistent, above the transcript, visible during editing too.
       Descriptive content — NOT role=alert, no modal, no repeated warning icons. -->
  {#if useStructuredView}
    <p class="transcript-caveat">
      Speaker labels are automatic and unverified. Check who spoke before attributing a quote or statement.
    </p>
  {/if}
  {#if showFailedStatus}
    <!-- 'failed' rendering: attempted and errored. Distinct banner from
         'skipped' (never ran) and from the unassigned sections (ran,
         couldn't attribute some spans). The persisted outcome must remain
         visible after reload — see contract line F.
         #108 a11y: the heading is a real <h3> (heading navigation, matching
         the unassigned section's <h4>) and the banner carries role="status"
         so assistive tech announces failed ≠ completed. role="status" is
         deliberate over role="alert": this is a persisted state on load,
         not an interrupting change. The skipped banner above is symmetric
         (same heading level, same role) — their parity is pinned by test.
         'completed' is deliberately NOT announced: success is signaled by
         the absence of any banner (decision recorded in issue #108). -->
    <div class="failed-status" data-testid="diarization-failed" role="status">
      <h3 class="failed-heading">Speaker labelling failed</h3>
      <p class="failed-reason">{failedReasonLine}</p>
    </div>
  {/if}
  {#if showSkippedStatus}
    <!-- 'skipped' rendering: requested but did not run. Distinct banner from
         'unknown' (metadata absent), 'off' (plain), and 'failed' (errored —
         renders via the structured/unassigned path). Supporting line and the
         Audio-settings affordance key off the persisted reason code only.
         Heading level + role symmetric with the failed banner (pinned). -->
    <div class="skipped-status" data-testid="diarization-skipped" role="status">
      <h3 class="skipped-heading">Speaker labelling wasn't run</h3>
      <p class="skipped-reason">{skippedReasonLine}</p>
      {#if onOpenAudioSettings}
        <button type="button" class="skipped-action" onclick={onOpenAudioSettings}>
          Open Audio settings
        </button>
      {/if}
    </div>
  {/if}
  <!-- #108 finding (b) / contract line G: the toolbar is OUTSIDE every
       outcome/body branch. Edit access must never depend on diarization
       outcome or body state — a legacy transcript with missing metadata
       ('unknown') must be correctable exactly like any other. -->
  {#if editing}
    <div class="edit-toolbar">
      <button class="btn-done" onclick={doneEdit}>Done</button>
    </div>
    <textarea
      bind:value={editText}
      {placeholder}
      class="editor-area"
    ></textarea>
  {:else}
    <div class="view-toolbar">
      <button class="btn-edit" onclick={startEdit}>Edit</button>
    </div>
    {#if showUnknownStatus}
      <!-- Honest 'unknown' rendering: metadata missing, no explicit labels in text.
           Shows unavailability message instead of silent plain text or false "Speaker unassigned". -->
      <div class="unknown-status">
        <p class="unknown-message">Speaker-labelling status unavailable</p>
        <p class="unknown-value">{value || placeholder}</p>
      </div>
    {:else if useStructuredView}
      <div class="sections">
        {#each sections as section, i (i)}
          {#if section.speaker}
            {@const slot = getSpeakerSlot(section.speaker)}
            <div class="speaker-section" style="border-left-color: {palette[slot].border}" aria-labelledby="speaker-{i}">
              <span id="speaker-{i}" class="speaker-badge slot-{slot}" style="background-color: {palette[slot].bg}; border-color: {palette[slot].border}">
                {section.speaker}
              </span>
              <p class="speaker-text">{section.text}</p>
            </div>
          {:else}
            <div class="speaker-section unlabeled" aria-labelledby="unlabeled-{i}">
              <h4 id="unlabeled-{i}" class="unlabeled-heading">Speaker unassigned</h4>
              <p class="speaker-text">{section.text}</p>
            </div>
          {/if}
        {/each}
      </div>
    {:else}
      <div class="plain-text">{value || placeholder}</div>
    {/if}
  {/if}
</div>

<style>
  .transcript-view {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  /* Uncertainty-honesty caveat: persistent, compact, above the scrolling
     transcript. Descriptive — not role=alert, no modal, no warning icons.
     Relies on explicit text + border styling (not color alone) for
     forced-colors mode and grayscale distinguishability. */
  .transcript-caveat {
    margin: 0;
    padding: 6px 16px;
    font-size: 12px;
    line-height: 1.4;
    color: var(--text-secondary);
    background-color: var(--bg-secondary);
    border-bottom: 1px solid var(--border-light);
    font-style: italic;
  }

  .view-toolbar,
  .edit-toolbar {
    display: flex;
    justify-content: flex-end;
    padding: 4px 16px;
    border-bottom: 1px solid var(--border-light);
    background-color: var(--bg-secondary);
  }

  .btn-edit,
  .btn-done {
    padding: 4px 12px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background-color 0.15s ease, color 0.15s ease;
  }

  .btn-edit:hover,
  .btn-done:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  .btn-done {
    color: var(--accent);
    border-color: var(--accent);
  }

  .sections {
    flex: 1;
    overflow-y: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .speaker-section {
    border-left: 3px solid var(--border);
    padding-left: 12px;
  }

  .speaker-section.unlabeled {
    /* #108 finding (c): the unassigned boundary was the least visible
       element (1.30:1 light / 1.51:1 dark with var(--border)). A 4px
       dashed --text-secondary rail measures 7.76:1 light / 4.84:1 dark
       against the production surfaces. The heading text itself is
       --text-primary, so strengthening the boundary never fades the words. */
    border-left: 4px dashed var(--text-secondary);
    padding-left: 12px;
  }

  /* Unlabeled heading: "Speaker unassigned" — #108 finding (c): raised from
     11px/--text-secondary to 13px/--text-primary (body-adjacent; body text
     is 14px, banner headings are 13px). Normal weight+colour, NOT
     faded/italic/hidden. Neutral styling (no speaker color).
     Relies on explicit text + dashed boundary (not color alone) for
     forced-colors mode and grayscale distinguishability. */
  .unlabeled-heading {
    margin: 0 0 4px 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
    letter-spacing: 0.02em;
    text-transform: none;
  }

  .speaker-badge {
    display: inline-block;
    font-size: 11px;
    font-weight: 600;
    padding: 2px 8px;
    border-radius: var(--radius-sm);
    border: 1px solid;
    margin-bottom: 4px;
    letter-spacing: 0.02em;
    /* Theme-aware per-slot text colour (see .slot-N below); falls back to
       full-contrast body text if a slot class is ever missing. */
    color: var(--slot-text, var(--text-primary));
  }

  /* #108 finding (c) / contract line H: badge text contrast. Values were
     computed (WCAG relative luminance, alpha-composited badge tint on
     --bg-primary and --bg-secondary in each theme) to clear 4.5:1 in BOTH
     themes — worst case ≥5:1. Light values are darkened variants of the
     palette hue; dark values are lightened variants where the base colour
     itself fails (Speaker 1/3/5/8 measured 4.06/3.82/4.28/4.16:1).
     TranscriptView.contrast.test.ts recomputes every ratio from these
     declarations — change a colour there, not in a parallel table.
     svelte-ignore css-unused-selector: the slot-N class is attached
     dynamically (class="speaker-badge slot-{slot}"), so Svelte cannot see
     these selectors are used. */
  /* svelte-ignore css-unused-selector */
  .slot-0 { --slot-text: #0a59db; }
  /* svelte-ignore css-unused-selector */
  .slot-1 { --slot-text: #0a7250; }
  /* svelte-ignore css-unused-selector */
  .slot-2 { --slot-text: #8715f4; }
  /* svelte-ignore css-unused-selector */
  .slot-3 { --slot-text: #8e5b06; }
  /* svelte-ignore css-unused-selector */
  .slot-4 { --slot-text: #bc1467; }
  /* svelte-ignore css-unused-selector */
  .slot-5 { --slot-text: #046e81; }
  /* svelte-ignore css-unused-selector */
  .slot-6 { --slot-text: #49710c; }
  /* svelte-ignore css-unused-selector */
  .slot-7 { --slot-text: #c30b2b; }
  :global([data-theme='dark']) .slot-0 { --slot-text: #6ba1f8; }
  :global([data-theme='dark']) .slot-1 { --slot-text: #10bb82; }
  :global([data-theme='dark']) .slot-2 { --slot-text: #c085f9; }
  :global([data-theme='dark']) .slot-3 { --slot-text: #f59e0b; }
  :global([data-theme='dark']) .slot-4 { --slot-text: #f176b2; }
  :global([data-theme='dark']) .slot-5 { --slot-text: #06b6d4; }
  :global([data-theme='dark']) .slot-6 { --slot-text: #84cc16; }
  :global([data-theme='dark']) .slot-7 { --slot-text: #f7758b; }

  .speaker-text {
    font-size: 14px;
    line-height: 1.6;
    color: var(--text-primary);
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .plain-text {
    flex: 1;
    overflow-y: auto;
    padding: 16px;
    font-size: 14px;
    line-height: 1.6;
    color: var(--text-primary);
    white-space: pre-wrap;
    word-break: break-word;
  }

  /* Honest 'unknown' rendering: metadata missing, no explicit labels in text.
     Shows unavailability message above the transcript content instead of
     silent plain text or false "Speaker unassigned". Neutral styling (no
     color-only indicator) for forced-colors mode and grayscale. */
  .unknown-status {
    flex: 1;
    overflow-y: auto;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .unknown-message {
    margin: 0;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
    padding: 8px 12px;
    background-color: var(--bg-secondary);
    border-left: 3px solid var(--border);
    border-radius: var(--radius-sm);
  }

  .unknown-value {
    margin: 0;
    font-size: 14px;
    line-height: 1.6;
    color: var(--text-primary);
    white-space: pre-wrap;
    word-break: break-word;
  }

  /* 'skipped' banner: requested but did not run. Visually and semantically
     distinct from the 'unknown' unavailability box: amber left rail (vs the
     unknown box's plain border), a heading + supporting line + affordance
     stack. Explicit text + border carry the distinction (not color alone)
     for forced-colors mode and grayscale. */
  .skipped-status {
    margin: 0;
    padding: 10px 12px;
    background-color: var(--bg-secondary);
    border-left: 3px solid var(--accent, #f59e0b);
    border-radius: var(--radius-sm);
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
  }

  .skipped-heading {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .skipped-reason {
    margin: 0;
    font-size: 12px;
    color: var(--text-secondary);
  }

  .skipped-action {
    margin-top: 4px;
    padding: 4px 12px;
    font-size: 12px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: background-color 0.15s ease, color 0.15s ease;
  }

  .skipped-action:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
  }

  /* 'failed' banner: attempted and errored. DELIBERATELY symmetric to
     .skipped-status / .skipped-heading / .skipped-reason — same padding,
     same type sizes, same weight (at-least-equal prominence; a persisted
     failure must not render subordinate to a skip notice). Only the rail
     colour differs (danger vs accent), and the distinction never rests on
     colour alone: the heading and reason text differ explicitly. */
  .failed-status {
    margin: 0;
    padding: 10px 12px;
    background-color: var(--bg-secondary);
    border-left: 3px solid var(--danger, #dc2626);
    border-radius: var(--radius-sm);
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
  }

  .failed-heading {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: var(--text-primary);
  }

  .failed-reason {
    margin: 0;
    font-size: 12px;
    color: var(--text-secondary);
  }

  .editor-area {
    flex: 1;
    width: 100%;
    resize: none;
    border: none;
    border-radius: 0;
    background-color: var(--bg-primary);
    color: var(--text-primary);
    font-size: 14px;
    line-height: 1.6;
    padding: 16px;
    outline: none;
    box-shadow: none;
    min-height: 0;
  }
</style>
