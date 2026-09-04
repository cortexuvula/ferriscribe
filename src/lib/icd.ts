/**
 * ICD code validation against the BC MSP-accepted list.
 *
 * The extraction in `rsvp/engine.ts` returns code strings that still
 * carry their `ICD-9:` / `ICD-9 Code:` prefix. This module strips the
 * prefix, normalizes the bare code (handles zero-padding differences),
 * and tests membership against the cached MSP set. It also resolves an
 * explaining title for each code — the note's own " — <description>"
 * text first (the SOAP output format emits one per code), falling back
 * to the official BC MSP description from the cached description map.
 */
import { extractIcdCodes } from './rsvp/engine';

export interface ValidatedIcdCode {
  /** Original extracted string, e.g. "ICD-9 Code: 401.9". */
  raw: string;
  /** Prefix-stripped code, e.g. "401.9" — what the chip displays. */
  bare: string;
  /** True if the code is on the MSP list, false if not, null if we
   *  couldn't validate (set not loaded yet). */
  valid: boolean | null;
  /** Explaining title for the code: the note's own description text when
   *  present, otherwise the official BC MSP description, null when
   *  neither is available (e.g. an ICD-10 code, or neither source
   *  loaded). */
  description: string | null;
}

/**
 * Per-line `ICD-9 Code: 847.2 — Sprain of lumbar` form the SOAP output
 * format mandates. Captures (code, description). Inline mentions like
 * "(ICD-9: 250.0)" carry no description and are intentionally unmatched —
 * their titles come from the MSP fallback map instead.
 */
const ICD_DESC_LINE_RE =
  /^[ \t]*ICD[-\s]?\d+(?:\s+Code)?[\s:—–-]+([A-Z]?[\d.]+[A-Z]?)[ \t]*[—–-][ \t]*(.+?)[ \t]*$/gimu;

/**
 * Collect the per-code descriptions embedded in note text. Keys are the
 * bare codes exactly as captured (first occurrence wins — a note repeats
 * a code line only when the model duplicated it).
 */
export function extractIcdDescriptions(text: string): Map<string, string> {
  const map = new Map<string, string>();
  for (const m of text.matchAll(ICD_DESC_LINE_RE)) {
    const code = m[1];
    const desc = m[2]?.trim();
    if (code && desc && !map.has(code)) map.set(code, desc);
  }
  return map;
}

/**
 * Soften the MSP source's ALL-CAPS descriptions ("LUMBAR") into title
 * case for the billing-code list. Note-written descriptions are already
 * sentence-cased and pass through untouched.
 */
function toDisplayCase(desc: string): string {
  return desc.toLowerCase().replace(/\b[a-z]/g, (c) => c.toUpperCase());
}

/**
 * Resolve the explaining title for a bare code: the note's own
 * description first, then the official MSP description. Both lookups try
 * the code's normalized forms (the note may emit "1.0" where the MSP
 * list keys "001.0").
 */
function resolveDescription(
  bare: string,
  noteDescs: ReadonlyMap<string, string>,
  mspDescriptions: ReadonlyMap<string, string> | null,
): string | null {
  for (const form of normalizedForms(bare)) {
    const note = noteDescs.get(form);
    if (note) return note;
  }
  if (mspDescriptions) {
    for (const form of normalizedForms(bare)) {
      const msp = mspDescriptions.get(form);
      if (msp) return toDisplayCase(msp);
    }
  }
  return null;
}

/**
 * Strip the `ICD-N` / `ICD-N Code:` prefix and surrounding brackets,
 * returning the bare code (e.g. "401.9", "V70.0", "01A").
 */
export function stripIcdPrefix(raw: string): string {
  return raw
    .replace(/^\(?\[?\s*ICD-\d+\s*(?:Code)?\s*:?\s*/iu, '')
    .replace(/^[()[\s]+|[)\]\s]+$/gu, '')
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
 *
 * `mspDescriptions` (the official code → description map from
 * `get_icd9_descriptions`) supplies explaining titles for codes whose
 * note text carries none; null/omitted skips the fallback.
 */
export function extractIcdCodesValidated(
  text: string,
  codeSet: Set<string> | null,
  mode: IcdMode = 'icd9',
  mspDescriptions: ReadonlyMap<string, string> | null = null,
): ValidatedIcdCode[] {
  const validateIcd9 = mode === 'icd9' || mode === 'both';
  const noteDescs = extractIcdDescriptions(text);
  return extractIcdCodes(text).map((raw) => {
    const bare = stripIcdPrefix(raw);
    const isIcd9 = /ICD-9/i.test(raw);
    // Only validate ICD-9 codes when we're in a mode that uses ICD-9,
    // and never validate ICD-10 codes (no bundled list). This prevents
    // the wrong-set false warning (e.g. an ICD-10 code validated
    // against the ICD-9 MSP list in pure icd10 mode).
    const valid =
      isIcd9 && validateIcd9 ? validateIcdCode(raw, codeSet) : null;
    return { raw, bare, valid, description: resolveDescription(bare, noteDescs, mspDescriptions) };
  });
}

/** Heading for the billing-code list, matching the configured ICD mode. */
export function billingCodesLabel(mode: IcdMode = 'icd9'): string {
  switch (mode) {
    case 'icd10':
      return 'Billing codes (ICD-10)';
    case 'both':
      return 'Billing codes (ICD-9/ICD-10)';
    default:
      return 'Billing codes (ICD-9)';
  }
}

// ─── metadata.icd_codes (structured codes, note body is code-free) ──────────

/**
 * One entry of `recordings.metadata.icd_codes` — written by the backend
 * at generation time when it strips the per-code ICD lines out of the
 * SOAP note. Mirrors the Rust `ExtractedIcdCode` wire shape
 * (`soap_generator::postprocess`); keep the two in sync.
 */
export interface IcdCodeMetadataEntry {
  code: string;
  description?: string | null;
  kind?: 'icd9' | 'icd10';
}

/**
 * Type guard for `metadata.icd_codes`. Returns `null` when the key is
 * absent (or not an array) — a legacy recording whose codes live inline
 * in the note text (mined via `extractIcdCodesValidated` as fallback).
 *
 * Individual entries are validated and malformed ones are DROPPED, not
 * fatal: the mapper calls `description?.trim()` and a non-string from
 * synced or corrupted metadata would crash the render. `kind` is
 * constrained to the wire vocabulary so a garbage value can't make an
 * ICD-10 entry validate against the ICD-9 MSP list.
 */
export function icdCodeMetadataEntries(metadata: unknown): IcdCodeMetadataEntry[] | null {
  if (typeof metadata !== 'object' || metadata === null) return null;
  const raw = (metadata as Record<string, unknown>)['icd_codes'];
  if (!Array.isArray(raw)) return null;
  return raw.flatMap((e): IcdCodeMetadataEntry[] => {
    if (typeof e !== 'object' || e === null) return [];
    const rec = e as Record<string, unknown>;
    const code = rec['code'];
    if (typeof code !== 'string' || code.length === 0) return [];
    const description = rec['description'];
    if (description !== undefined && description !== null && typeof description !== 'string') {
      return [];
    }
    const kind = rec['kind'];
    if (kind !== undefined && kind !== 'icd9' && kind !== 'icd10') return [];
    return [{ code, description: description as string | null | undefined, kind }];
  });
}

/**
 * Map structured metadata codes to the list's `ValidatedIcdCode` rows.
 * Same mode/validation rules as `extractIcdCodesValidated`: ICD-9 codes
 * validate against the MSP set in icd9/both modes; ICD-10 codes render
 * neutral. Titles come from the entry's description, with the official
 * MSP description as fallback.
 *
 * Rows are keyed by code (first occurrence wins; a later duplicate only
 * backfills a missing description) — the Rust two-pass extractor could
 * emit the same code twice when it appeared both standalone and mid-line,
 * and metadata arriving via sync from an older peer can still carry
 * duplicates. Without keying, the billing list rendered the code twice.
 */
export function icdCodesFromMetadata(
  entries: IcdCodeMetadataEntry[],
  codeSet: Set<string> | null,
  mode: IcdMode = 'icd9',
  mspDescriptions: ReadonlyMap<string, string> | null = null,
): ValidatedIcdCode[] {
  const validateIcd9 = mode === 'icd9' || mode === 'both';
  const byCode = new Map<string, ValidatedIcdCode>();
  for (const entry of entries) {
    const bare = entry.code;
    const isIcd10 = entry.kind === 'icd10';
    const valid =
      !isIcd10 && validateIcd9 ? validateIcdCode(bare, codeSet) : null;
    const description =
      entry.description?.trim() ||
      resolveDescription(bare, new Map(), mspDescriptions);
    const row: ValidatedIcdCode = {
      raw: `ICD-${isIcd10 ? '10' : '9'} Code: ${bare}`,
      bare,
      valid,
      description,
    };
    const existing = byCode.get(bare);
    if (!existing) {
      byCode.set(bare, row);
    } else if (!existing.description && row.description) {
      existing.description = row.description;
    }
  }
  return [...byCode.values()];
}

/**
 * Resolve the billing codes for a recording: from `metadata.icd_codes`
 * when present (new-format recordings — the note body is code-free),
 * otherwise by mining the note text (legacy recordings whose codes are
 * inline). `noteText` may be null when only metadata is at hand.
 */
export function resolveIcdCodes(
  metadata: unknown,
  noteText: string | null,
  codeSet: Set<string> | null,
  mode: IcdMode = 'icd9',
  mspDescriptions: ReadonlyMap<string, string> | null = null,
): ValidatedIcdCode[] {
  const entries = icdCodeMetadataEntries(metadata);
  if (entries) return icdCodesFromMetadata(entries, codeSet, mode, mspDescriptions);
  return noteText ? extractIcdCodesValidated(noteText, codeSet, mode, mspDescriptions) : [];
}
