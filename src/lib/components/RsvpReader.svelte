<script lang="ts">
import { onMount, onDestroy } from 'svelte';
import { rsvp } from '../stores/rsvp.svelte';
import { settings } from '../stores/settings.svelte';
import {
  tokenize,
  orpIndex,
  baseDelayMs,
  delayMs,
  type Token,
} from '../rsvp/engine';

let tokens = $state<Token[]>([]);
let index = $state(0);
let playing = $state(false);
let timerHandle: ReturnType<typeof setTimeout> | null = null;
let autoStartHandle: ReturnType<typeof setTimeout> | null = null;

$effect(() => {
  if (!rsvp.state.reader.open) return;
  tokens = tokenize(rsvp.state.reader.text);
  index = 0;
  playing = false;
  clearTimer();
  clearAutoStart();
  if (settings.state.rsvp_auto_start) {
    autoStartHandle = setTimeout(() => {
      autoStartHandle = null;
      // Only kick off if the reader is still open and we haven't already started.
      if (rsvp.state.reader.open && !playing) play();
    }, 500);
  }
});

function currentWords(): Token[] {
  const chunk = settings.state.rsvp_chunk_size || 1;
  return tokens.slice(index, index + chunk);
}

function splitOrp(word: string): { pre: string; orp: string; post: string } {
  const i = orpIndex(word);
  return {
    pre: word.slice(0, i),
    orp: word[i] ?? '',
    post: word.slice(i + 1),
  };
}

function scheduleNext(): void {
  if (!playing) return;
  if (index >= tokens.length) {
    playing = false;
    return;
  }
  const chunk = settings.state.rsvp_chunk_size || 1;
  const midToken = tokens[Math.min(index + Math.floor(chunk / 2), tokens.length - 1)];
  const ms = delayMs(midToken, baseDelayMs(settings.state.rsvp_wpm || 300));
  timerHandle = setTimeout(() => {
    index = Math.min(index + chunk, tokens.length);
    scheduleNext();
  }, ms);
}

function clearTimer(): void {
  if (timerHandle !== null) {
    clearTimeout(timerHandle);
    timerHandle = null;
  }
}

function clearAutoStart(): void {
  if (autoStartHandle !== null) {
    clearTimeout(autoStartHandle);
    autoStartHandle = null;
  }
}

function play(): void {
  clearAutoStart();
  if (playing) return;
  if (index >= tokens.length) index = 0;
  playing = true;
  scheduleNext();
}

function pause(): void {
  if (!playing) return;
  playing = false;
  clearTimer();
}

function togglePlay(): void {
  if (playing) pause(); else play();
}

function stepForward(): void {
  pause();
  const chunk = settings.state.rsvp_chunk_size || 1;
  index = Math.min(index + chunk, tokens.length);
}

function stepBack(): void {
  pause();
  const chunk = settings.state.rsvp_chunk_size || 1;
  index = Math.max(0, index - chunk);
}

function goHome(): void {
  pause();
  index = 0;
}

function goEnd(): void {
  pause();
  index = tokens.length;
}

function bumpWpm(delta: number): void {
  const next = Math.max(50, Math.min(2000, (settings.state.rsvp_wpm || 300) + delta));
  settings.updateField('rsvp_wpm', next);
}

function setChunk(n: number): void {
  settings.updateField('rsvp_chunk_size', n);
}

function toggleTheme(): void {
  settings.updateField('rsvp_dark_theme', !settings.state.rsvp_dark_theme);
}

function close(): void {
  clearAutoStart();
  pause();
  rsvp.closeAll();
}

function onKeydown(e: KeyboardEvent): void {
  if (!rsvp.state.reader.open) return;
  switch (e.key) {
    case ' ': e.preventDefault(); togglePlay(); break;
    case 'ArrowLeft': e.preventDefault(); stepBack(); break;
    case 'ArrowRight': e.preventDefault(); stepForward(); break;
    case 'ArrowUp': e.preventDefault(); bumpWpm(25); break;
    case 'ArrowDown': e.preventDefault(); bumpWpm(-25); break;
    case 'Home': e.preventDefault(); goHome(); break;
    case 'End': e.preventDefault(); goEnd(); break;
    case '1': setChunk(1); break;
    case '2': setChunk(2); break;
    case '3': setChunk(3); break;
    case 't':
    case 'T':
      toggleTheme();
      break;
    case 'Escape':
      e.preventDefault();
      close();
      break;
  }
}

onMount(() => {
  window.addEventListener('keydown', onKeydown);
});

onDestroy(() => {
  window.removeEventListener('keydown', onKeydown);
  clearTimer();
  clearAutoStart();
});

let stageFont = $derived(
  (settings.state.rsvp_font_size ?? 48) - ((settings.state.rsvp_chunk_size ?? 1) > 1 ? 8 : 0),
);

let progressPct = $derived(
  tokens.length === 0 ? 0 : Math.round((index / tokens.length) * 100),
);

let etaSecs = $derived(
  Math.max(
    0,
    Math.round(((tokens.length - index) * 60) / (settings.state.rsvp_wpm || 300)),
  ),
);

function formatEta(secs: number): string {
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}
</script>

{#if rsvp.state.reader.open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <div
    class="backdrop"
    class:dark={settings.state.rsvp_dark_theme}
    onclick={close}
    role="presentation"
  >
    <div class="dialog" role="dialog" aria-modal="true" tabindex="-1" onclick={(e) => e.stopPropagation()}>
      <div class="header">
        <h2>Speed Read</h2>
        <button class="btn-close" aria-label="Close" onclick={close}>&times;</button>
      </div>

      <div class="stage" style="font-size: {stageFont}px;">
        {#if tokens.length === 0}
          <span class="empty">Nothing to read.</span>
        {:else if index >= tokens.length}
          <span class="empty">Done.</span>
        {:else}
          {#each currentWords() as tok, i}
            {#if i === Math.floor(currentWords().length / 2)}
              {@const parts = splitOrp(tok.word)}
              <span class="word orp-word">
                <span class="pre">{parts.pre}</span><span class="orp">{parts.orp}</span><span class="post">{parts.post}</span>
              </span>
            {:else}
              <span class="word">{tok.word}</span>
            {/if}
          {/each}
        {/if}
      </div>

      <div class="progress-row">
        <div class="bar"><div class="fill" style="width: {progressPct}%"></div></div>
        <span class="count">{index} / {tokens.length}</span>
        <span class="eta">{formatEta(etaSecs)}</span>
      </div>

      <div class="controls">
        <button onclick={goHome} title="Restart (Home)">⏮</button>
        <button onclick={stepBack} title="Previous (←)">◀</button>
        <button onclick={togglePlay} title="Play / Pause (Space)">
          {playing ? '❚❚' : '▶'}
        </button>
        <button onclick={stepForward} title="Next (→)">▶</button>
        <button onclick={goEnd} title="Skip to end (End)">⏭</button>

        <div class="sep"></div>

        <label title="Words per minute (↑/↓)">
          WPM
          <input
            type="range"
            min="50"
            max="2000"
            step="25"
            value={settings.state.rsvp_wpm}
            onchange={(e) => settings.updateField('rsvp_wpm', Number((e.currentTarget as HTMLInputElement).value))}
          />
          <span class="num">{settings.state.rsvp_wpm}</span>
        </label>

        <label title="Font size">
          Font
          <input
            type="range"
            min="24"
            max="96"
            step="2"
            value={settings.state.rsvp_font_size}
            onchange={(e) => settings.updateField('rsvp_font_size', Number((e.currentTarget as HTMLInputElement).value))}
          />
          <span class="num">{settings.state.rsvp_font_size}</span>
        </label>

        <div class="chunk-group" title="Chunk size (1/2/3)">
          {#each [1, 2, 3] as n}
            <button
              class:active={settings.state.rsvp_chunk_size === n}
              onclick={() => setChunk(n)}
            >{n}</button>
          {/each}
        </div>

        <button onclick={toggleTheme} title="Theme (T)">
          {settings.state.rsvp_dark_theme ? '☀' : '☾'}
        </button>
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.8);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1600;
  }
  .dialog {
    background: var(--bg-primary);
    color: var(--text-primary);
    border-radius: var(--radius-lg);
    padding: 24px 32px;
    width: min(900px, 95vw);
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    gap: 18px;
  }
  .backdrop.dark .dialog {
    background: #111;
    color: #eee;
  }
  .header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .btn-close {
    background: none;
    border: none;
    color: inherit;
    font-size: 1.4rem;
    line-height: 1;
    cursor: pointer;
    padding: 4px 8px;
    border-radius: 4px;
  }
  .btn-close:hover { background: rgba(255,255,255,0.1); }

  .stage {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 24px;
    min-height: 180px;
    font-family: 'SF Mono', Menlo, Consolas, monospace;
    font-weight: 700;
    letter-spacing: 0.02em;
  }
  .word { display: inline-flex; }
  .orp-word .pre, .orp-word .post { opacity: 0.95; }
  .orp-word .orp { color: #ef4444; }

  .empty {
    color: var(--text-secondary);
    font-size: 1rem;
    font-weight: 400;
    font-family: inherit;
  }

  .progress-row {
    display: flex;
    align-items: center;
    gap: 12px;
    font-size: 0.85rem;
    color: var(--text-secondary);
  }
  .bar {
    flex: 1;
    height: 4px;
    background: rgba(255,255,255,0.1);
    border-radius: 2px;
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--accent, #4a9eff);
    transition: width 0.1s linear;
  }

  .controls {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }
  .controls button {
    background: var(--bg-secondary);
    color: inherit;
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 10px;
    cursor: pointer;
    font-size: 0.95rem;
    min-width: 36px;
  }
  .controls button:hover { background: var(--bg-hover); }
  .controls button.active {
    background: var(--accent, #4a9eff);
    color: white;
    border-color: var(--accent, #4a9eff);
  }
  .controls label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 0.85rem;
    color: var(--text-secondary);
  }
  /* Override the app's global `input { width: 100% }` so the sliders size
     to their intended width instead of blowing out the toolbar. */
  .controls input[type="range"] {
    width: 140px;
    flex: 0 0 auto;
  }
  .controls .num { min-width: 3ch; text-align: right; }
  .chunk-group { display: inline-flex; gap: 2px; }
  .chunk-group button { border-radius: 0; }
  .chunk-group button:first-child { border-top-left-radius: 6px; border-bottom-left-radius: 6px; }
  .chunk-group button:last-child { border-top-right-radius: 6px; border-bottom-right-radius: 6px; }
  .sep {
    width: 1px;
    height: 24px;
    background: var(--border);
    margin: 0 4px;
  }
</style>
