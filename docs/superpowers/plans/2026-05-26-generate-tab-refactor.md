# GenerateTab Component Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the 593-line GenerateTab.svelte into three focused components for better readability, testability, and maintainability.

**Architecture:** Extract context input UI into ContextPanel.svelte (~240 lines), generation controls into GenerateControls.svelte (~240 lines), and keep orchestration in GenerateTab.svelte (~110 lines). All state is lifted to GenerateTab, child components are presentational.

**Tech Stack:** Svelte 5, TypeScript, Vitest for testing

---

### Task 1: Extract ContextPanel component

**Files:**
- Create: `src/lib/components/ContextPanel.svelte`
- Modify: `src/lib/pages/GenerateTab.svelte`

- [ ] **Step 1: Create ContextPanel.svelte with props interface**

Create `src/lib/components/ContextPanel.svelte`:

```svelte
<script lang="ts">
  interface Props {
    medicationsText: string;
    allergiesText: string;
    conditionsText: string;
    contextText: string;
    expanded: boolean;
    hasActiveContext: boolean;
    onToggle: () => void;
    onInsertTemplate: (text: string) => void;
    onClearContext: () => void;
    onMedicationsChange: (value: string) => void;
    onAllergiesChange: (value: string) => void;
    onConditionsChange: (value: string) => void;
    onContextChange: (value: string) => void;
  }

  let {
    medicationsText,
    allergiesText,
    conditionsText,
    contextText,
    expanded,
    hasActiveContext,
    onToggle,
    onInsertTemplate,
    onClearContext,
    onMedicationsChange,
    onAllergiesChange,
    onConditionsChange,
    onContextChange,
  }: Props = $props();

  const CONTEXT_TEMPLATES = [
    { label: 'Follow-up', text: 'Follow-up visit for ongoing condition. Previous visit findings:\n\n' },
    { label: 'New Patient', text: 'New patient consultation. No prior history available.\n\n' },
    { label: 'Lab Results', text: 'Recent lab results:\n- \n- \n- \n\n' },
    { label: 'Referral Info', text: 'Referred by: \nReason for referral: \nRelevant history: \n\n' },
  ];
</script>

<div class="context-panel" class:expanded>
  <button class="context-toggle" onclick={onToggle}>
    <span class="toggle-arrow">{expanded ? '▾' : '▸'}</span>
    <span class="toggle-label">Additional Context</span>
    {#if hasActiveContext}
      <span class="context-badge">Active</span>
    {/if}
  </button>

  {#if expanded}
    <div class="context-body">
      <p class="context-hint">
        Add medications, allergies, and known conditions as structured lists below. Use the Notes textarea for everything else (lab values, prior visit narrative, family/social history, etc.).
      </p>

      <label class="field-label" for="ctx-medications">Medications (one per line)</label>
      <textarea
        id="ctx-medications"
        class="context-textarea structured"
        placeholder="Lisinopril 10mg PO daily"
        value={medicationsText}
        oninput={(e) => onMedicationsChange(e.currentTarget.value)}
        rows="3"
      ></textarea>

      <label class="field-label" for="ctx-allergies">Allergies (one per line)</label>
      <textarea
        id="ctx-allergies"
        class="context-textarea structured"
        placeholder="Penicillin (rash)"
        value={allergiesText}
        oninput={(e) => onAllergiesChange(e.currentTarget.value)}
        rows="2"
      ></textarea>

      <label class="field-label" for="ctx-conditions">Known conditions (one per line)</label>
      <textarea
        id="ctx-conditions"
        class="context-textarea structured"
        placeholder="Type 2 diabetes"
        value={conditionsText}
        oninput={(e) => onConditionsChange(e.currentTarget.value)}
        rows="3"
      ></textarea>

      <label class="field-label" for="ctx-notes">Notes</label>
      <div class="context-templates">
        {#each CONTEXT_TEMPLATES as tmpl}
          <button class="template-chip" onclick={() => onInsertTemplate(tmpl.text)}>
            {tmpl.label}
          </button>
        {/each}
      </div>
      <textarea
        id="ctx-notes"
        class="context-textarea"
        placeholder="Free-form notes (lab values, prior visit narrative, family/social history)..."
        value={contextText}
        oninput={(e) => onContextChange(e.currentTarget.value)}
        rows="6"
      ></textarea>
      {#if contextText.trim()}
        <button class="context-clear" onclick={onClearContext}>
          Clear notes
        </button>
      {/if}
    </div>
  {/if}
</div>
```

- [ ] **Step 2: Add CSS to ContextPanel.svelte**

Add to bottom of `src/lib/components/ContextPanel.svelte`:

```svelte
<style>
  .context-panel {
    margin-bottom: 16px;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background-color: var(--bg-card);
    overflow: hidden;
  }

  .context-toggle {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    padding: 10px 14px;
    font-size: 13px;
    font-weight: 500;
    color: var(--text-secondary);
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
    transition: color 0.15s ease;
  }

  .context-toggle:hover {
    color: var(--text-primary);
  }

  .toggle-arrow {
    font-size: 11px;
    color: var(--text-muted);
  }

  .toggle-label {
    flex: 1;
  }

  .context-badge {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--accent);
    background-color: color-mix(in srgb, var(--accent) 15%, transparent);
    border: 1px solid color-mix(in srgb, var(--accent) 30%, transparent);
    border-radius: var(--radius-sm);
    padding: 1px 6px;
  }

  .context-body {
    padding: 0 14px 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .context-hint {
    font-size: 12px;
    color: var(--text-muted);
    line-height: 1.5;
    margin: 0;
  }

  .context-templates {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .template-chip {
    padding: 4px 10px;
    font-size: 11px;
    font-weight: 500;
    color: var(--text-secondary);
    background-color: var(--bg-tertiary, #374151);
    border: 1px solid var(--border);
    border-radius: 12px;
    cursor: pointer;
    transition: background-color 0.15s ease, color 0.15s ease, border-color 0.15s ease;
  }

  .template-chip:hover {
    background-color: var(--bg-hover);
    color: var(--text-primary);
    border-color: var(--accent);
  }

  .context-textarea {
    width: 100%;
    resize: vertical;
    min-height: 80px;
    padding: 10px;
    font-size: 13px;
    font-family: inherit;
    line-height: 1.5;
    color: var(--text-primary);
    background-color: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    transition: border-color 0.15s ease;
  }

  .field-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    margin-top: 4px;
    margin-bottom: -4px;
  }

  .context-textarea.structured {
    min-height: 56px;
  }

  .context-textarea::placeholder {
    color: var(--text-muted);
  }

  .context-textarea:focus {
    outline: none;
    border-color: var(--accent);
  }

  .context-clear {
    align-self: flex-end;
    padding: 4px 10px;
    font-size: 11px;
    font-weight: 500;
    color: var(--text-muted);
    background: none;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    cursor: pointer;
    transition: color 0.15s ease, border-color 0.15s ease;
  }

  .context-clear:hover {
    color: var(--danger, #ef4444);
    border-color: var(--danger, #ef4444);
  }
</style>
```

- [ ] **Step 3: Update GenerateTab.svelte to use ContextPanel**

In `src/lib/pages/GenerateTab.svelte`:

1. Add import at top (after line 7):

```typescript
import ContextPanel from '../components/ContextPanel.svelte';
```

2. Replace lines 161-225 (context panel template) with:

```svelte
      <ContextPanel
        {medicationsText}
        {allergiesText}
        {conditionsText}
        {contextText}
        expanded={contextExpanded}
        {hasActiveContext}
        onToggle={() => (contextExpanded = !contextExpanded)}
        onInsertTemplate={insertTemplate}
        onClearContext={() => (contextText = '')}
        onMedicationsChange={(value) => (medicationsText = value)}
        onAllergiesChange={(value) => (allergiesText = value)}
        onConditionsChange={(value) => (conditionsText = value)}
        onContextChange={(value) => (contextText = value)}
      />
```

3. Remove context panel CSS (lines 366-507):
   - Remove `.context-panel` and all its child selectors
   - Keep `.error-banner`, `.progress-banner`, `.generate-buttons`, `.letter-controls` styles

- [ ] **Step 4: Run tests to verify no regressions**

```bash
npm run test
```

Expected: All tests pass

- [ ] **Step 5: Run svelte-check**

```bash
npm run check
```

Expected: No errors or warnings

- [ ] **Step 6: Commit ContextPanel extraction**

```bash
git add src/lib/components/ContextPanel.svelte src/lib/pages/GenerateTab.svelte
git commit -m "refactor(ui): extract ContextPanel from GenerateTab

Move context input UI (medications, allergies, conditions, notes) into
dedicated ContextPanel component. This separates presentation from
orchestration and makes the context input logic independently testable.

- Create ContextPanel.svelte with props interface
- Update GenerateTab to use ContextPanel component
- Lift all context state to GenerateTab (presentational pattern)"
```

---

### Task 2: Extract GenerateControls component

**Files:**
- Create: `src/lib/components/GenerateControls.svelte`
- Modify: `src/lib/pages/GenerateTab.svelte`

- [ ] **Step 1: Create GenerateControls.svelte with props interface**

Create `src/lib/components/GenerateControls.svelte`:

```svelte
<script lang="ts">
  import type { Recording } from '../types';
  import type { LetterAudience } from '../stores/letterAudiences.svelte';
  import GenerateItem from './GenerateItem.svelte';

  interface Props {
    recording: Recording | null;
    generationState: {
      generating: 'soap' | 'referral' | 'letter' | null;
      error: string | null;
      progressStatus: string | null;
    };
    copyStatus: Record<string, 'idle' | 'copying' | 'copied'>;
    selectedAudienceId: string | null;
    letterType: string;
    audiences: LetterAudience[];
    onGenerate: (type: 'soap' | 'referral' | 'letter') => void;
    onCopy: (type: string) => void;
    onSpeedRead: (type: string) => void;
    onClearError: () => void;
    onAudienceChange: (id: string | null) => void;
    onLetterTypeChange: (type: string) => void;
  }

  let {
    recording,
    generationState,
    copyStatus,
    selectedAudienceId,
    letterType,
    audiences,
    onGenerate,
    onCopy,
    onSpeedRead,
    onClearError,
    onAudienceChange,
    onLetterTypeChange,
  }: Props = $props();
</script>

{#if generationState.error}
  <div class="error-banner">
    <span>{generationState.error}</span>
    <button class="error-dismiss" onclick={onClearError}>
      Dismiss
    </button>
  </div>
{/if}

{#if generationState.progressStatus}
  <div class="progress-banner">{generationState.progressStatus}</div>
{/if}

<div class="generate-buttons">
  <GenerateItem
    title="SOAP Note"
    description="Structured clinical note (Subjective, Objective, Assessment, Plan)"
    generating={generationState.generating === 'soap'}
    anyGenerating={generationState.generating !== null}
    done={!!recording?.soap_note}
    copyStatus={copyStatus['soap']}
    onGenerate={() => onGenerate('soap')}
    onCopy={() => onCopy('soap')}
    onSpeedRead={() => onSpeedRead('soap')}
  />
  <GenerateItem
    title="Referral Letter"
    description="Specialist referral letter based on the consultation"
    generating={generationState.generating === 'referral'}
    anyGenerating={generationState.generating !== null}
    done={!!recording?.referral}
    copyStatus={copyStatus['referral']}
    onGenerate={() => onGenerate('referral')}
    onCopy={() => onCopy('referral')}
    onSpeedRead={() => onSpeedRead('referral')}
  />
  <div class="letter-controls">
    <label class="field-label" for="letter-audience">Audience</label>
    <select
      id="letter-audience"
      class="letter-select"
      value={selectedAudienceId ?? ''}
      onchange={(e) => onAudienceChange(e.currentTarget.value || null)}
    >
      {#each audiences as audience}
        <option value={audience.id}>{audience.name}</option>
      {/each}
    </select>

    <label class="field-label" for="letter-type">Letter purpose</label>
    <input
      id="letter-type"
      type="text"
      class="letter-input"
      placeholder="e.g. follow-up, pre-authorization"
      value={letterType}
      oninput={(e) => onLetterTypeChange(e.currentTarget.value)}
    />
  </div>
  <GenerateItem
    title="Letter"
    description={selectedAudienceId
      ? (() => {
          const a = audiences.find((x) => x.id === selectedAudienceId);
          return a ? `Letter for ${a.name}` : 'Letter';
        })()
      : 'Letter'}
    generating={generationState.generating === 'letter'}
    anyGenerating={generationState.generating !== null}
    done={!!recording?.letter}
    copyStatus={copyStatus['letter']}
    onGenerate={() => onGenerate('letter')}
    onCopy={() => onCopy('letter')}
    onSpeedRead={() => onSpeedRead('letter')}
  />
</div>
```

- [ ] **Step 2: Add CSS to GenerateControls.svelte**

Add to bottom of `src/lib/components/GenerateControls.svelte`:

```svelte
<style>
  .error-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 12px;
    margin-bottom: 16px;
    background-color: rgba(239, 68, 68, 0.1);
    border: 1px solid var(--danger, #ef4444);
    border-radius: var(--radius-md);
    font-size: 13px;
    color: var(--danger, #ef4444);
  }

  .error-dismiss {
    padding: 2px 8px;
    border-radius: var(--radius-sm);
    font-size: 12px;
    color: var(--danger, #ef4444);
    border: 1px solid var(--danger, #ef4444);
    background: transparent;
    cursor: pointer;
  }

  .error-dismiss:hover {
    background-color: var(--danger, #ef4444);
    color: white;
  }

  .progress-banner {
    padding: 8px 12px;
    margin-bottom: 16px;
    background-color: rgba(59, 130, 246, 0.1);
    border: 1px solid var(--accent, #3b82f6);
    border-radius: var(--radius-md);
    font-size: 13px;
    color: var(--accent, #3b82f6);
  }

  .generate-buttons {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .letter-controls {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    background-color: var(--bg-card);
  }

  .letter-select,
  .letter-input {
    width: 100%;
    height: 34px;
    padding: 0 10px;
    font-size: 13px;
    font-family: inherit;
    color: var(--text-primary);
    background-color: var(--bg-primary);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    transition: border-color 0.15s ease;
    box-sizing: border-box;
  }

  .letter-select {
    appearance: auto;
    cursor: pointer;
  }

  .letter-input::placeholder {
    color: var(--text-muted);
  }

  .letter-select:focus,
  .letter-input:focus {
    outline: none;
    border-color: var(--accent);
  }

  .field-label {
    font-size: 11px;
    font-weight: 600;
    color: var(--text-secondary);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    margin-top: 4px;
    margin-bottom: -4px;
  }
</style>
```

- [ ] **Step 3: Update GenerateTab.svelte to use GenerateControls**

In `src/lib/pages/GenerateTab.svelte`:

1. Add import at top (after ContextPanel import):

```typescript
import GenerateControls from '../components/GenerateControls.svelte';
```

2. Replace lines 227-298 (error banner, progress banner, generate buttons) with:

```svelte
      <GenerateControls
        recording={recordings.selectedRecording}
        generationState={generation.state}
        {copyStatus}
        {selectedAudienceId}
        {letterType}
        audiences={letterAudiences.audiences}
        onGenerate={handleGenerate}
        onCopy={handleCopy}
        onSpeedRead={handleSpeedRead}
        onClearError={() => generation.clearError()}
        onAudienceChange={(id) => (selectedAudienceId = id)}
        onLetterTypeChange={(type) => (letterType = type)}
      />
```

3. Remove generation controls CSS (lines 509-593):
   - Remove `.error-banner` and all its child selectors
   - Remove `.progress-banner` and all its child selectors
   - Remove `.generate-buttons` and all its child selectors
   - Remove `.letter-controls`, `.letter-select`, `.letter-input` and all their child selectors

- [ ] **Step 4: Run tests to verify no regressions**

```bash
npm run test
```

Expected: All tests pass

- [ ] **Step 5: Run svelte-check**

```bash
npm run check
```

Expected: No errors or warnings

- [ ] **Step 6: Commit GenerateControls extraction**

```bash
git add src/lib/components/GenerateControls.svelte src/lib/pages/GenerateTab.svelte
git commit -m "refactor(ui): extract GenerateControls from GenerateTab

Move generation controls UI (SOAP/Referral/Letter buttons, letter options)
into dedicated GenerateControls component. This separates presentation
from orchestration and makes the generation UI independently testable.

- Create GenerateControls.svelte with props interface
- Update GenerateTab to use GenerateControls component
- Move error and progress banners to GenerateControls"
```

---

### Task 3: Clean up and verify line counts

**Files:**
- Verify: `src/lib/pages/GenerateTab.svelte` (~110 lines)
- Verify: `src/lib/components/ContextPanel.svelte` (~240 lines)
- Verify: `src/lib/components/GenerateControls.svelte` (~240 lines)

- [ ] **Step 1: Count lines in each component**

```bash
wc -l src/lib/pages/GenerateTab.svelte src/lib/components/ContextPanel.svelte src/lib/components/GenerateControls.svelte
```

Expected output:
- GenerateTab.svelte: ~110 lines
- ContextPanel.svelte: ~240 lines
- GenerateControls.svelte: ~240 lines

- [ ] **Step 2: Review GenerateTab.svelte for unused code**

Read `src/lib/pages/GenerateTab.svelte` and verify:
- No unused imports
- No unused state variables
- No orphaned CSS
- Clear separation of concerns (state management + business logic + component composition)

- [ ] **Step 3: Run full test suite**

```bash
npm run test
```

Expected: All tests pass

- [ ] **Step 4: Run svelte-check**

```bash
npm run check
```

Expected: No errors or warnings

- [ ] **Step 5: Commit final cleanup**

```bash
git add -A
git commit -m "refactor(ui): complete GenerateTab component split

Final state after splitting 593-line GenerateTab.svelte:
- ContextPanel.svelte: ~240 lines (context input fields)
- GenerateControls.svelte: ~240 lines (generation buttons + letter options)
- GenerateTab.svelte: ~110 lines (orchestration + state management)

All tests pass, svelte-check clean. This improves readability,
testability, and maintainability by separating presentation from
orchestration and lifting state to the parent component."
```

---

## Success Criteria

- [ ] `GenerateTab.svelte` reduced to ~110 lines (from 593)
- [ ] `ContextPanel.svelte` contains only context input UI (~240 lines)
- [ ] `GenerateControls.svelte` contains only generation controls (~240 lines)
- [ ] All state lifted to GenerateTab - child components are presentational
- [ ] All tests pass: `npm run test`
- [ ] No svelte-check warnings: `npm run check`
- [ ] Git history shows clear incremental progression (3 commits)

## Notes

- Each task maintains passing tests - never commit broken state
- Child components are purely presentational - all business logic stays in GenerateTab
- Props interfaces are explicit - no hidden coupling between components
- The refactoring is purely structural - no behavior changes
- No new dependencies needed - all existing utilities and stores are reused
