// @vitest-environment jsdom
/**
 * sanitizedErr — the PHI-safe error interpolation for user-facing toasts
 * (trash-restore polish Items 1+2: backend error strings must never be
 * interpolated raw; a filename is patient-visible surface).
 *
 * Facts pinned:
 *   - A `recording <uuid>` reference (the db layer's NotFound wording) is
 *     KEPT — ids are opaque, never patient names/content. Works for Error
 *     instances, plain strings, and the serialized AppError `{kind,
 *     message}` shape that crosses the Tauri invoke boundary.
 *   - Everything else — including messages that embed filenames or other
 *     row data — collapses to the generic string.
 *   - The exact toast texts the trash surfaces render with it.
 */
import { describe, it, expect } from 'vitest';
import { sanitizedErr, GENERIC_ERROR_TEXT } from './sanitizedErr';

const UUID = '3fa85f64-5717-4562-b3fc-2c963f66afa6';

describe('sanitizedErr — keeps the safe id reference', () => {
  it('keeps `recording <uuid>` from an Error instance', () => {
    expect(sanitizedErr(new Error(`Not found: recording ${UUID}`))).toBe(
      `recording ${UUID}`,
    );
  });

  it('keeps `recording <uuid>` from a plain string rejection', () => {
    expect(sanitizedErr(`recording ${UUID} is deleted`)).toBe(`recording ${UUID}`);
  });

  it('keeps `recording <uuid>` from the serialized AppError object shape', () => {
    // Tauri rejects the promise with AppError's Serialize output.
    const appErr = { kind: 'Database', message: `Not found: recording ${UUID}` };
    expect(sanitizedErr(appErr)).toBe(`recording ${UUID}`);
  });
});

describe('sanitizedErr — everything else collapses to the generic string', () => {
  it('a filename-bearing message never reaches the glass', () => {
    expect(sanitizedErr('failed to open Smith_John_2026-10-06.wav')).toBe(
      GENERIC_ERROR_TEXT,
    );
  });

  it('Error objects without an id reference collapse', () => {
    expect(sanitizedErr(new Error('database is locked'))).toBe(GENERIC_ERROR_TEXT);
  });

  it('non-string, non-Error junk collapses (no throw)', () => {
    expect(sanitizedErr(undefined)).toBe(GENERIC_ERROR_TEXT);
    expect(sanitizedErr(null)).toBe(GENERIC_ERROR_TEXT);
    expect(sanitizedErr(42)).toBe(GENERIC_ERROR_TEXT);
    expect(sanitizedErr({ kind: 'Other' })).toBe(GENERIC_ERROR_TEXT);
  });

  it('store-thrown UI strings collapse too (undoMoveAll guard throws)', () => {
    expect(sanitizedErr(new Error('No move-all to undo'))).toBe(GENERIC_ERROR_TEXT);
    expect(sanitizedErr(new Error('A restore is already in progress'))).toBe(
      GENERIC_ERROR_TEXT,
    );
  });

  it('does not match an id reference embedded in a larger word', () => {
    expect(sanitizedErr(`avrecording ${UUID}`)).toBe(GENERIC_ERROR_TEXT);
  });
});

describe('sanitizedErr — the exact toast texts the trash surfaces render', () => {
  it('RecordingsTab / TrashPanel interpolations', () => {
    expect(`Could not restore: ${sanitizedErr(new Error('db busy'))}`).toBe(
      `Could not restore: ${GENERIC_ERROR_TEXT}`,
    );
    expect(`Failed to move recording to Trash: ${sanitizedErr('locked')}`).toBe(
      `Failed to move recording to Trash: ${GENERIC_ERROR_TEXT}`,
    );
    expect(
      `Failed to move recordings to Trash: ${sanitizedErr(new Error('pool timeout'))}`,
    ).toBe(`Failed to move recordings to Trash: ${GENERIC_ERROR_TEXT}`);
    expect(`Couldn't restore recording: ${sanitizedErr(`recording ${UUID}`)}`).toBe(
      `Couldn't restore recording: recording ${UUID}`,
    );
    expect(`Couldn't restore recordings: ${sanitizedErr('io error')}`).toBe(
      `Couldn't restore recordings: ${GENERIC_ERROR_TEXT}`,
    );
  });
});
