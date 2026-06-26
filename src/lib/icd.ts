/**
 * ICD code validation against the BC MSP-accepted list.
 *
 * The extraction in `rsvp/engine.ts` returns code strings that still
 * carry their `ICD-9:` / `ICD-9 Code:` prefix. This module strips the
 * prefix, normalizes the bare code (handles zero-padding differences),
 * and tests membership against the cached MSP set.
 */
import { extractIcdCodes } from './rsvp/engine';

export interface ValidatedIcdCode {
  /** Original extracted string, e.g. "ICD-9 Code: 401.9". */
  raw: string;
  /** True if the code is on the MSP list, false if not, null if we
   *  couldn't validate (set not loaded yet). */
  valid: boolean | null;
}

/**
 * Strip the `ICD-N` / `ICD-N Code:` prefix and surrounding brackets,
 * returning the bare code (e.g. "401.9", "V70.0", "01A").
 */
export function stripIcdPrefix(raw: string): string {
  return raw
    .replace(/^\(?\[?\s*ICD-\d+\s*(?:Code)?\s*:?\s*/iu, '')
    .replace(/^[\(\[\s]+|[\)\]\s]+$/gu, '')
    .trim();
}

/**
 * Normalize a bare code for membership comparison.
 *
 * The MSP list uses zero-padded numeric codes (`001.0`, `042`), but a
 * model may emit a trimmed form (`1.0`, `42`). This returns the set of
 * candidate forms to try (original + zero-padded) so callers can test
 * membership with any of them. Alpha-suffixed MSP codes (`01A`) and
 * V/E codes are returned unchanged.
 */
export function normalizedForms(code: string): string[] {
  const trimmed = code.trim();
  const forms = [trimmed];
  // Pure-numeric codes: zero-pad the integer part to 3 digits.
  const dotIdx = trimmed.indexOf('.');
  let padded: string | undefined;
  if (dotIdx > 0) {
    const intPart = trimmed.slice(0, dotIdx);
    const rest = trimmed.slice(dotIdx);
    const n = Number(intPart);
    if (Number.isInteger(n)) padded = String(n).padStart(3, '0') + rest;
  } else {
    const n = Number(trimmed);
    if (Number.isInteger(n)) padded = String(n).padStart(3, '0');
  }
  // Only include the padded form when it differs — avoids duplicates like
  // ["780","780"] for already-3-digit numeric codes.
  if (padded !== undefined && padded !== trimmed) forms.push(padded);
  return forms;
}

/**
 * Validate a raw extracted code against the MSP set.
 *
 * @returns `true` if the bare code is on the list, `false` if it is
 *   not, or `null` if the set isn't loaded yet (can't validate), or if
 *   the stripped bare code contains no digit (a stray prose match like
 *   "ICD-9." that the loose extraction regex picks up — these render
 *   neutral rather than as a false billing warning).
 */
export function validateIcdCode(
  raw: string,
  codeSet: Set<string> | null,
): boolean | null {
  if (!codeSet) return null;
  const bare = stripIcdPrefix(raw);
  if (!bare) return null;
  // Stray prose guard: a valid ICD code always contains at least one
  // digit. Matches like "ICD-9." (no code body) are extraction noise.
  if (!/\d/.test(bare)) return null;
  return normalizedForms(bare).some((f) => codeSet.has(f));
}

/** ICD coding mode — mirrors the Rust IcdVersion enum (snake_case). */
export type IcdMode = 'icd9' | 'icd10' | 'both';

/**
 * Extract and validate ICD codes from note text.
 *
 * Mode-aware: in `icd9` mode, ICD-9 codes validate against the MSP set
 * and any stray ICD-10 codes (model mislabel) render neutral without a
 * false warning. In `icd10` mode, ICD-10 codes render neutral (no
 * bundled ICD-10 list) and ICD-9 codes are NOT validated against the
 * wrong set. In `both` mode, ICD-9 codes validate, ICD-10 codes render
 * neutral. Defaults to `icd9` (the BC MSP billing standard).
 */
export function extractIcdCodesValidated(
  text: string,
  codeSet: Set<string> | null,
  mode: IcdMode = 'icd9',
): ValidatedIcdCode[] {
  const validateIcd9 = mode === 'icd9' || mode === 'both';
  return extractIcdCodes(text).map((raw) => {
    const isIcd9 = /ICD-9/i.test(raw);
    // Only validate ICD-9 codes when we're in a mode that uses ICD-9,
    // and never validate ICD-10 codes (no bundled list). This prevents
    // the wrong-set false warning (e.g. an ICD-10 code validated
    // against the ICD-9 MSP list in pure icd10 mode).
    if (isIcd9 && validateIcd9) return { raw, valid: validateIcdCode(raw, codeSet) };
    return { raw, valid: null };
  });
}
