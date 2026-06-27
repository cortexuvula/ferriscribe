<script lang="ts">
  interface SpeakerSection {
    speaker: string | null;
    text: string;
  }

  interface Props {
    value?: string;
    /** Structured segment data from recording metadata — preferred over text parsing. */
    segments?: Array<{ speaker: string | null; text: string; start: number; end: number }>;
    placeholder?: string;
    onChange?: (v: string) => void;
  }

  const { value = '', segments, placeholder = '', onChange = () => {} }: Props = $props();

  let editing = $state(false);
  let editText = $state(value);

  // Re-sync editText when the external value changes (e.g. different recording selected).
  $effect(() => {
    editText = value;
  });

  // Debounce the derived parse so typing into the transcript editor doesn't
  // re-parse the entire transcript on every keystroke. The parent (EditorTab)
  // optimistically updates `value` per keystroke; without this debounce, a
  // long transcript would jank the editor. 400ms feels instant to the user
  // but batches rapid typing.
  let parseTimer: ReturnType<typeof setTimeout> | null = null;
  let debouncedValue = $state(value);
  $effect(() => {
    // Read `value` inside the effect so the dependency is tracked. Then
    // defer the assignment into a timeout to batch rapid changes.
    const current = value;
    if (parseTimer !== null) clearTimeout(parseTimer);
    parseTimer = setTimeout(() => {
      debouncedValue = current;
    }, 400);
  });

  // Parse the transcript into speaker sections.
  // Uses structured segments from metadata when available (more reliable),
  // falls back to regex parsing of "Speaker N: text" formatted text.
  const sections: SpeakerSection[] = $derived.by(() => {
    if (segments && segments.length > 0) {
      return groupSegmentsIntoSections(segments);
    }
    return parseTextSections(debouncedValue);
  });

  const hasSpeakers = $derived(sections.some((s) => s.speaker !== null));

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
      const match = para.match(/^(Speaker \d+):\s*([\s\S]*)$/);
      if (match) {
        result.push({ speaker: match[1], text: match[2] });
      } else if (para.trim()) {
        // No speaker label — could be unlabeled text before first speaker
        // or text that was edited to remove labels.
        result.push({ speaker: null, text: para.trim() });
      }
    }
    return result;
  }

  // Deterministic color per speaker — hash the speaker label to a hue.
  const speakerColors = new Map<string, string>();
  const palette = [
    { bg: 'rgba(59, 130, 246, 0.12)', border: '#3b82f6', text: '#3b82f6' },  // blue
    { bg: 'rgba(16, 185, 129, 0.12)', border: '#10b981', text: '#10b981' },  // emerald
    { bg: 'rgba(168, 85, 247, 0.12)', border: '#a855f7', text: '#a855f7' },  // purple
    { bg: 'rgba(245, 158, 11, 0.12)', border: '#f59e0b', text: '#f59e0b' },  // amber
    { bg: 'rgba(236, 72, 153, 0.12)', border: '#ec4899', text: '#ec4899' },  // pink
    { bg: 'rgba(6, 182, 212, 0.12)', border: '#06b6d4', text: '#06b6d4' },  // cyan
    { bg: 'rgba(132, 204, 22, 0.12)', border: '#84cc16', text: '#84cc16' },  // lime
    { bg: 'rgba(244, 63, 94, 0.12)', border: '#f43f5e', text: '#f43f5e' },  // rose
  ];

  function getSpeakerColor(speaker: string): { bg: string; border: string; text: string } {
    let idx = speakerColors.get(speaker);
    if (!idx) {
      const n = speakerColors.size;
      idx = String(n % palette.length);
      speakerColors.set(speaker, idx);
    }
    return palette[parseInt(idx)];
  }

  function startEdit() {
    editText = value;
    editing = true;
  }

  function doneEdit() {
    onChange(editText);
    editing = false;
  }
</script>

<div class="transcript-view">
  {#if editing}
    <div class="edit-toolbar">
      <button class="btn-done" onclick={doneEdit}>Done</button>
    </div>
    <textarea
      bind:value={editText}
      {placeholder}
      class="editor-area"
    ></textarea>
  {:else if hasSpeakers}
    <div class="view-toolbar">
      <button class="btn-edit" onclick={startEdit}>Edit</button>
    </div>
    <div class="sections">
      {#each sections as section}
        {#if section.speaker}
          {@const colors = getSpeakerColor(section.speaker)}
          <div class="speaker-section" style="border-left-color: {colors.border}">
            <span class="speaker-badge" style="background-color: {colors.bg}; color: {colors.text}; border-color: {colors.border}">
              {section.speaker}
            </span>
            <p class="speaker-text">{section.text}</p>
          </div>
        {:else}
          <div class="speaker-section unlabeled">
            <p class="speaker-text">{section.text}</p>
          </div>
        {/if}
      {/each}
    </div>
  {:else}
    <div class="view-toolbar">
      <button class="btn-edit" onclick={startEdit}>Edit</button>
    </div>
    <div class="plain-text">{value || placeholder}</div>
  {/if}
</div>

<style>
  .transcript-view {
    flex: 1;
    display: flex;
    flex-direction: column;
    overflow: hidden;
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
    border-left-color: var(--border-light);
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
  }

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
