# GenerateTab Component Refactor Design

**Date:** 2026-05-26  
**Status:** Draft  
**Priority:** Low (Code Organization)  
**Estimated effort:** 2-3 hours

## Problem Statement

`src/lib/pages/GenerateTab.svelte` has grown to 593 lines, mixing three distinct concerns:

- **Context input** (240 lines): Structured fields (medications, allergies, conditions) + freeform notes with templates
- **Generation controls** (240 lines): SOAP/Referral/Letter buttons, letter audience+type inputs
- **Orchestration** (110 lines): Business logic, API calls, state management, error handling

This makes the file harder to navigate, test, and maintain. The UI presentation logic is tightly coupled to business logic, preventing reuse and granular testing.

## Goals

1. **Readability** — Separate presentation from orchestration
2. **Testability** — Enable isolated unit testing of UI components
3. **Maintainability** — Reduce cognitive load when working on any single concern
4. **Reusability** — Make ContextPanel potentially reusable in other contexts

## Proposed Solution: Three-Component Split

Split `GenerateTab.svelte` into three focused components with lifted state management.

### 1. `ContextPanel.svelte` — Context Input (~240 lines)

**Responsibility:** Present and manage context input fields (medications, allergies, conditions, notes).

**Public API:**
```typescript
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
```

**Contents:**
- Collapsible panel UI (toggle, badge, body)
- Four textarea fields (medications, allergies, conditions, notes)
- Template chips (Follow-up, New Patient, Lab Results, Referral Info)
- Clear button for notes
- All CSS for context panel styling

**Why separate:**
- Pure presentational component with clear inputs/outputs
- Contains UI logic that could be reused elsewhere (e.g., other forms)
- Can be tested without API calls or generation state
- Single responsibility: context input UI

### 2. `GenerateControls.svelte` — Generation Controls (~240 lines)

**Responsibility:** Present generation buttons and letter-specific options.

**Public API:**
```typescript
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
  onAudienceChange: (id: string | null) => void;
  onLetterTypeChange: (type: string) => void;
}
```

**Contents:**
- Three GenerateItem components (SOAP, Referral, Letter)
- Letter-specific controls (audience select, purpose input)
- Error and progress banners
- All CSS for generation controls styling

**Why separate:**
- Encapsulates all generation UI concerns
- Complex state presentation (generating/copying/done states)
- Can be tested with mock props (no real API calls needed)
- Clear boundary: "everything about triggering generation"

### 3. `GenerateTab.svelte` — Orchestration (~110 lines)

**Responsibility:** Orchestrate context input, generation triggers, and state management.

**Contents:**
- State management for all context fields
- State management for copy status tracking
- State management for letter options
- Business logic handlers:
  - `handleGenerate()` - API calls with error handling
  - `handleCopy()` - clipboard operations
  - `handleSpeedRead()` - RSVP reader integration
  - `insertTemplate()` - context template insertion
- Recording metadata synchronization ($effect)
- Letter audience loading ($effect)
- Component composition (ContextPanel + GenerateControls)

**Dependencies:**
- Renders `ContextPanel` with context state and callbacks
- Renders `GenerateControls` with generation state and callbacks
- Uses stores: `recordings`, `generation`, `letterAudiences`, `rsvp`
- Uses utilities: `generateSoap`, `generateReferral`, `generateLetter`, `copyWithStatus`, `buildPatientContext`

## Component Interactions

```
GenerateTab.svelte (orchestration)
    │
    ├─> ContextPanel.svelte (presentation)
    │       Props: context state + callbacks
    │       Emits: user actions (toggle, insert, clear, change)
    │
    ├─> GenerateControls.svelte (presentation)
    │       Props: recording + generation state + callbacks
    │       Emits: user actions (generate, copy, speedRead)
    │
    └─> Local business logic
            - API calls (generateSoap, etc.)
            - State updates
            - Error handling
```

The orchestration component holds state and passes slices of it to child components:
- Context fields: `medicationsText`, `allergiesText`, `conditionsText`, `contextText`
- UI state: `contextExpanded`, `copyStatus`
- Letter options: `selectedAudienceId`, `letterType`
- Derived state: `hasActiveContext`

## Migration Strategy

Use incremental migration to minimize risk and maintain functionality at each step.

### Step 1: Extract ContextPanel (1-1.5 hours)

1. Create `src/lib/components/ContextPanel.svelte`
2. Move context input template (lines 161-225)
3. Move context panel CSS (lines 366-507)
4. Define Props interface with all context fields
5. Replace inline template with `<ContextPanel>` component
6. Pass state and callbacks as props
7. Run `npm run test` to verify no regressions
8. Commit: "refactor(ui): extract ContextPanel from GenerateTab"

### Step 2: Extract GenerateControls (1-1.5 hours)

1. Create `src/lib/components/GenerateControls.svelte`
2. Move generation controls template (lines 238-298)
3. Move error/progress banners (lines 227-236)
4. Move generation controls CSS (lines 548-593)
5. Define Props interface with generation state and callbacks
6. Replace inline template with `<GenerateControls>` component
7. Pass state and callbacks as props
8. Run `npm run test` to verify no regressions
9. Commit: "refactor(ui): extract GenerateControls from GenerateTab"

### Step 3: Clean up and verify (0.5 hours)

1. Review GenerateTab.svelte - should be ~110 lines
2. Review imports and remove unused imports
3. Run `npm run check` (svelte-check)
4. Run `npm run test`
5. Commit: "refactor(ui): finalize GenerateTab component split"

## Acceptance Criteria

- [ ] `GenerateTab.svelte` reduced to ~110 lines (from 593)
- [ ] `ContextPanel.svelte` contains only context input UI (~240 lines)
- [ ] `GenerateControls.svelte` contains only generation controls (~240 lines)
- [ ] All state lifted to GenerateTab - child components are presentational
- [ ] `npm run test` passes with no failures
- [ ] `npm run check` passes with no warnings
- [ ] No changes to public API or behavior
- [ ] Git history shows clear incremental progression

## Risks and Mitigations

### Risk 1: Breaking changes to component behavior
**Mitigation:** Child components are purely presentational - all business logic stays in GenerateTab. Incremental migration with tests after each step.

### Risk 2: Prop drilling becomes unwieldy
**Mitigation:** Props are well-defined and grouped by concern (context fields, generation state, letter options). Consider grouping into objects if needed.

### Risk 3: Event handling complexity
**Mitigation:** Use callback props (onGenerate, onCopy, etc.) instead of Svelte events. This makes the interface explicit and testable.

### Risk 4: Over-engineering
**Mitigation:** Three-component split is the minimum viable refactor. Could stop after Step 1 (ContextPanel extraction) if that's sufficient.

## Alternatives Considered

### Alternative A: Two-component split (context + everything else)
- **Pros:** Smaller change, lower risk
- **Cons:** Still leaves a 350-line file, misses separation of generation UI
- **Verdict:** Doesn't achieve goals

### Alternative B: Four-component split (per-document generators)
- **Pros:** Maximum granularity
- **Cons:** Letter generator has unique controls (audience/type), others don't. Over-engineering for current needs.
- **Verdict:** Not needed for current use case

### Alternative C: Keep state local to components
- **Pros:** More encapsulated components
- **Cons:** Harder to coordinate state (e.g., recording metadata sync), more complex testing
- **Verdict:** Lifted state is clearer and more maintainable

## Success Metrics

1. **Line count reduction:** `GenerateTab.svelte` drops from 593 to ~110 lines (81% reduction)
2. **Test isolation:** Each component can be tested independently with mock props
3. **Code navigation:** Finding context or generation logic requires opening one file instead of searching
4. **Reusability:** `ContextPanel` can be used in other forms without modification

## References

- Current implementation: `src/lib/pages/GenerateTab.svelte`
- Component patterns: `src/lib/components/GenerateItem.svelte`
- State management: `src/lib/stores/generation.svelte`, `src/lib/stores/letterAudiences.svelte`
- API calls: `src/lib/api/generation.ts`
