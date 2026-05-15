<script lang="ts">
  import { testVocabularyCorrection, type CorrectionResult } from '../api/vocabulary';
  import { formatError } from '../types/errors';

  interface Props {
    resetSignal: number;
  }

  let { resetSignal }: Props = $props();

  let testInput = $state('');
  let testResult = $state<CorrectionResult | null>(null);
  let testError = $state<string | null>(null);
  let testing = $state(false);

  $effect(() => {
    resetSignal; // reactive read
    testResult = null;
    testError = null;
  });

  async function handleTest() {
    if (!testInput.trim()) return;
    testError = null;
    testing = true;
    try {
      testResult = await testVocabularyCorrection(testInput);
    } catch (err) {
      console.error('Test failed:', err);
      testError = formatError(err) || 'Test failed.';
    } finally {
      testing = false;
    }
  }
</script>

<div class="vocab-test">
  <h3>Test Corrections</h3>
  <textarea
    bind:value={testInput}
    placeholder="Paste sample text to test corrections..."
    rows="3"
  ></textarea>
  <button class="btn-test" onclick={handleTest} disabled={!testInput.trim() || testing}>
    {testing ? 'Testing...' : 'Test'}
  </button>
  {#if testError}
    <div class="test-error">{testError}</div>
  {/if}
  {#if testResult}
    <div class="test-result">
      <strong>{testResult.total_replacements} replacement{testResult.total_replacements !== 1 ? 's' : ''}</strong>
      <pre>{testResult.corrected_text}</pre>
    </div>
  {/if}
</div>

<style>
  .vocab-test {
    flex: 0 0 auto;
    padding: 12px 20px 16px;
    border-top: 1px solid var(--border-color, #333);
    background: var(--bg-primary, #111);
    max-height: 40vh;
    overflow-y: auto;
  }
  .vocab-test h3 { margin: 0 0 8px; font-size: 0.9rem; font-weight: 600; }
  .vocab-test textarea {
    width: 100%;
    padding: 8px;
    border-radius: 4px;
    border: 1px solid var(--border-color, #444);
    background: var(--bg-secondary, #1e1e1e);
    color: var(--text-primary, #e0e0e0);
    resize: vertical;
    font-family: inherit;
    font-size: 0.88rem;
    box-sizing: border-box;
  }
  .btn-test {
    margin-top: 8px;
    padding: 6px 14px;
    border-radius: 4px;
    border: none;
    background: var(--accent-color, #4a9eff);
    color: white;
    cursor: pointer;
    font-size: 0.88rem;
  }
  .btn-test:disabled { opacity: 0.5; cursor: not-allowed; }
  .btn-test:not(:disabled):hover { filter: brightness(1.1); }
  .test-error {
    margin-top: 10px;
    padding: 8px 10px;
    border-radius: 4px;
    background: rgba(255, 107, 107, 0.1);
    color: #ff6b6b;
    font-size: 0.85rem;
    border: 1px solid rgba(255, 107, 107, 0.3);
    word-wrap: break-word;
  }
  .test-result { margin-top: 10px; }
  .test-result strong { font-size: 0.85rem; color: var(--accent-color, #4a9eff); }
  .test-result pre {
    background: var(--bg-secondary, #1e1e1e);
    padding: 10px;
    border-radius: 4px;
    white-space: pre-wrap;
    word-wrap: break-word;
    font-size: 0.85rem;
    margin-top: 6px;
    border: 1px solid var(--border-color, #333);
  }
</style>
