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
  if (dotIdx > 0) {
    const intPart = trimmed.slice(0, dotIdx);
    const rest = trimmed.slice(dotIdx);
    const n = Number(intPart);
    if (Number.isInteger(n)) forms.push(String(n).padStart(3, '0') + rest);
  } else {
    const n = Number(trimmed);
    if (Number.isInteger(n)) forms.push(String(n).padStart(3, '0'));
  }
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

/**
 * Extract and validate ICD codes from note text.
 *
 * Wraps `extractIcdCodes` so existing call sites can swap one call.
 * Only ICD-9 codes are validated against the MSP ICD-9 set; ICD-10
 * codes (which appear in "both" mode) have no bundled list and render
 * as `valid: null` (neutral).
 */
export function extractIcdCodesValidated(
  text: string,
  codeSet: Set<string> | null,
): ValidatedIcdCode[] {
  return extractIcdCodes(text).map((raw) => ({
    raw,
    valid: /ICD-9/i.test(raw) ? validateIcdCode(raw, codeSet) : null,
  }));
}
