// @vitest-environment jsdom
/**
 * Prompts.svelte — the prompt editor must show the prompt that is actually
 * in effect: custom > selected specialty pack (assembled, exactly what
 * generation sends) > built-in default. Regression for the pane that kept
 * showing the built-in default after selecting a specialty pack.
 */
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/svelte';
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import Prompts from './Prompts.svelte';
import type { SpecialtyPackInfo } from '../../api/specialty';
import { toasts } from '../../stores/toasts.svelte';
import { confirmDialog } from '../../stores/confirm.svelte';

const mockState = vi.hoisted(() => ({
  specialty: null as string | null,
  custom_soap_prompt: null as string | null,
  custom_referral_prompt: null as string | null,
  custom_letter_prompt: null as string | null,
  custom_synopsis_prompt: null as string | null,
  custom_peer_discussion_prompt: null as string | null,
}));

const mockUpdateField = vi.hoisted(() => vi.fn(async () => {}));
const mockListPacks = vi.hoisted(() =>
  vi.fn(async (): Promise<SpecialtyPackInfo[]> => []),
);
const mockGetPackPrompt = vi.hoisted(() => vi.fn(async (): Promise<string | null> => null));
const mockGetDefaultPrompt = vi.hoisted(
  () => vi.fn(async (docType: string) => `DEFAULT PROMPT (${docType})`),
);

vi.mock('../../stores/settings.svelte', () => ({
  settings: { state: mockState, updateField: mockUpdateField },
}));
vi.mock('../../api/prompts', () => ({
  getDefaultPrompt: mockGetDefaultPrompt,
}));
vi.mock('../../api/specialty', () => ({
  listSpecialtyPacks: mockListPacks,
  getSpecialtyPackPrompt: mockGetPackPrompt,
}));
vi.mock('../../stores/toasts.svelte', () => ({
  toasts: { error: vi.fn(), success: vi.fn(), info: vi.fn() },
}));
vi.mock('../../stores/confirm.svelte', () => ({
  confirmDialog: vi.fn(async () => true),
}));

const PSYCHIATRY = {
  id: 'psychiatry',
  name: 'Psychiatry',
  version: '1.0.0',
  description: 'Psychiatric SOAP variant',
  icon: null,
  source: 'bundled' as const,
  provided_prompts: ['soap' as const],
};

const ASSEMBLED_PSYCHIATRY = 'PSYCHIATRY PACK BODY\n\n---\n\nSAFETY RULES — locked block';

function textarea(): HTMLTextAreaElement {
  return screen.getByRole('textbox') as HTMLTextAreaElement;
}

function resetState() {
  mockState.specialty = null;
  mockState.custom_soap_prompt = null;
  mockState.custom_referral_prompt = null;
  mockState.custom_letter_prompt = null;
  mockState.custom_synopsis_prompt = null;
  mockState.custom_peer_discussion_prompt = null;
}

describe('Prompts editor prompt source', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetState();
    mockListPacks.mockResolvedValue([]);
  });
  afterEach(cleanup);

  it('shows the built-in default when no specialty is selected', async () => {
    render(Prompts);
    await waitFor(() => expect(textarea().value).toBe('DEFAULT PROMPT (soap)'));
    expect(mockGetDefaultPrompt).toHaveBeenCalledWith('soap');
    expect(mockGetPackPrompt).not.toHaveBeenCalled();
    expect(screen.getByText('default')).toBeTruthy();
  });

  it('shows the assembled pack prompt once the selected pack serves the doc type', async () => {
    mockState.specialty = 'psychiatry';
    mockListPacks.mockResolvedValue([PSYCHIATRY]);
    mockGetPackPrompt.mockResolvedValue(ASSEMBLED_PSYCHIATRY);

    render(Prompts);
    // The pack list arrives async — the editor must settle on the PACK
    // prompt (with the safety block), not stay on the default it may have
    // shown for a frame.
    await waitFor(() =>
      expect(textarea().value).toBe(ASSEMBLED_PSYCHIATRY),
    );
    expect(mockGetPackPrompt).toHaveBeenCalledWith('soap');
    expect(textarea().value).not.toContain('DEFAULT PROMPT');
    // Status line + explanatory note name the pack.
    expect(screen.getByText('Psychiatry (+ safety block)')).toBeTruthy();
    expect(screen.getByText(/Psychiatry pack prompt is shown above/)).toBeTruthy();
  });

  it('falls back to the built-in default for doc types the pack does not provide', async () => {
    mockState.specialty = 'psychiatry';
    mockListPacks.mockResolvedValue([PSYCHIATRY]);

    render(Prompts);
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Clinical Synopsis' })).toBeTruthy(),
    );
    await fireEvent.click(screen.getByRole('button', { name: 'Clinical Synopsis' }));
    await waitFor(() => expect(textarea().value).toBe('DEFAULT PROMPT (synopsis)'));
    // Psychiatry only provides soap — no pack fetch for synopsis.
    expect(mockGetPackPrompt).not.toHaveBeenCalledWith('synopsis');
  });

  it('still shows the custom prompt when one is saved (custom beats the pack)', async () => {
    mockState.specialty = 'psychiatry';
    mockState.custom_soap_prompt = 'MY CUSTOM SOAP PROMPT';
    mockListPacks.mockResolvedValue([PSYCHIATRY]);

    render(Prompts);
    await waitFor(() => expect(textarea().value).toBe('MY CUSTOM SOAP PROMPT'));
    expect(mockGetPackPrompt).not.toHaveBeenCalled();
    expect(screen.getByText('custom')).toBeTruthy();
    // The override warning names the pack (arrives with the pack list).
    await waitFor(() =>
      expect(screen.getByText(/overrides the Psychiatry pack/)).toBeTruthy(),
    );
  });

  it('saving edits to the displayed pack text creates a custom override', async () => {
    mockState.specialty = 'psychiatry';
    mockListPacks.mockResolvedValue([PSYCHIATRY]);
    mockGetPackPrompt.mockResolvedValue(ASSEMBLED_PSYCHIATRY);

    render(Prompts);
    await waitFor(() => expect(textarea().value).toBe(ASSEMBLED_PSYCHIATRY));

    await fireEvent.input(textarea(), {
      target: { value: `${ASSEMBLED_PSYCHIATRY}\nEDITED LINE` },
    });
    await fireEvent.click(screen.getByRole('button', { name: 'Save as custom' }));
    await waitFor(() =>
      expect(mockUpdateField).toHaveBeenCalledWith(
        'custom_soap_prompt',
        `${ASSEMBLED_PSYCHIATRY}\nEDITED LINE`,
      ),
    );
  });

  it('a reset confirmed after switching prompt types clears the custom but never loads the old doc under the new heading', async () => {
    mockState.custom_soap_prompt = 'MY CUSTOM SOAP PROMPT';

    // Hold the reset confirmation open while the user switches doc type.
    let resolveConfirm!: (v: boolean) => void;
    const confirmHeld = new Promise<boolean>((resolve) => {
      resolveConfirm = resolve;
    });
    vi.mocked(confirmDialog).mockImplementationOnce(() => confirmHeld);

    render(Prompts);
    await waitFor(() => expect(textarea().value).toBe('MY CUSTOM SOAP PROMPT'));

    // Reset (soap) → dialog pending → switch to synopsis while it is open.
    await fireEvent.click(screen.getByRole('button', { name: 'Reset to default' }));
    await fireEvent.click(screen.getByRole('button', { name: 'Clinical Synopsis' }));
    await waitFor(() => expect(textarea().value).toBe('DEFAULT PROMPT (synopsis)'));

    resolveConfirm(true);
    await waitFor(() =>
      expect(mockUpdateField).toHaveBeenCalledWith('custom_soap_prompt', null),
    );
    // The editor must still show the SYNOPSIS prompt — not reload soap.
    await new Promise((r) => setTimeout(r, 25));
    expect(textarea().value).toBe('DEFAULT PROMPT (synopsis)');
  });

  it('surfaces a load failure instead of a silently blank editor', async () => {
    mockState.specialty = 'psychiatry';
    mockListPacks.mockResolvedValue([PSYCHIATRY]);
    mockGetPackPrompt.mockRejectedValue(
      new Error('Specialty pack "psychiatry" soap prompt too large: 60000 chars'),
    );

    render(Prompts);
    await waitFor(() =>
      expect(vi.mocked(toasts.error)).toHaveBeenCalledWith(
        expect.stringContaining('Could not load the prompt'),
      ),
    );
    // The editor lands in the explicit failure state (blank + toast), not a
    // half-loaded prompt pretending to be the pack.
    expect(textarea().value).toBe('');
  });
});
