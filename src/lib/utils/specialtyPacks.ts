/**
 * Pure helpers for the specialty prompt-pack picker: which prompt source is
 * ACTIVE for a doc type, and where the custom-prompt conflict lives.
 *
 * Precedence (must mirror the Rust builders):
 *   custom free-text > selected specialty pack (+ safety block) > built-in
 *   default. For letters, an explicitly chosen audience beats everything
 *   (decided per generation call, not visible here).
 */

import type { DocType } from '../api/prompts';
import type { SpecialtyPackInfo } from '../api/specialty';

/** The per-doc-type `custom_*_prompt` config keys, parallel to DocType. */
export const CUSTOM_PROMPT_FIELD: Record<DocType, string> = {
  soap: 'custom_soap_prompt',
  referral: 'custom_referral_prompt',
  letter: 'custom_letter_prompt',
  synopsis: 'custom_synopsis_prompt',
  peer_discussion: 'custom_peer_discussion_prompt',
};

/** The pack that will actually serve `doc` under the selected specialty id
 * (user packs override bundled packs with the same id on the backend, which
 * has already been applied before the list reaches the UI). */
export function activePack(
  packs: SpecialtyPackInfo[],
  specialty: string | null | undefined,
): SpecialtyPackInfo | null {
  const id = specialty?.trim();
  if (!id) return null;
  return packs.find((p) => p.id === id && !p.error) ?? null;
}

/** True when the pack provides a prompt artifact for this doc type (falls
 * back per-artifact to the built-in default otherwise). */
export function packProvides(pack: SpecialtyPackInfo | null, doc: DocType): boolean {
  return pack?.provided_prompts.includes(doc) ?? false;
}

/** The prompt source actually in effect for a doc type right now:
 * - `custom` — the user's free-text override wins outright;
 * - `pack` — the selected pack's artifact (+ the compiled-in safety block);
 * - `default` — the built-in prompt (for `default`, the family-medicine
 *   pack on the SOAP path). */
export type PromptSource = 'custom' | 'pack' | 'default';

export function activeSource(
  doc: DocType,
  customPrompt: string | null | undefined,
  pack: SpecialtyPackInfo | null,
): PromptSource {
  if (customPrompt && customPrompt.length > 0) return 'custom';
  if (packProvides(pack, doc)) return 'pack';
  return 'default';
}

/** Human label for the "Using:" status line. */
export function sourceLabel(source: PromptSource, pack: SpecialtyPackInfo | null): string {
  switch (source) {
    case 'custom':
      return 'custom';
    case 'pack':
      return `${pack?.name ?? 'specialty pack'} (+ safety block)`;
    case 'default':
      return 'default';
  }
}

/** The doc types where a stored custom prompt SILENTLY overrides the
 * selected pack — surfaced in Settings as the conflict the design requires
 * us to flag. */
export function conflictingDocTypes(
  customPrompts: Partial<Record<DocType, string | null | undefined>>,
  pack: SpecialtyPackInfo | null,
): DocType[] {
  const conflicts: DocType[] = [];
  for (const doc of Object.keys(CUSTOM_PROMPT_FIELD) as DocType[]) {
    if (activeSource(doc, customPrompts[doc], pack) === 'custom' && packProvides(pack, doc)) {
      conflicts.push(doc);
    }
  }
  return conflicts;
}
