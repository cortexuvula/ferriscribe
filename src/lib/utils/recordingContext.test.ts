import { describe, it, expect } from 'vitest';
import { contextFromMetadata } from './recordingContext';
import type { Recording } from '../types';

describe('contextFromMetadata', () => {
  it('returns all-empty fields for null metadata', () => {
    expect(contextFromMetadata(null)).toEqual({
      contextText: '',
      medicationsText: '',
      allergiesText: '',
      conditionsText: '',
    });
  });

  it('returns all-empty fields for undefined metadata', () => {
    expect(contextFromMetadata(undefined)).toEqual({
      contextText: '',
      medicationsText: '',
      allergiesText: '',
      conditionsText: '',
    });
  });

  it('returns all-empty fields for a non-object (array) metadata', () => {
    // Defensive: an array is typeof 'object' but not a valid metadata bag.
    expect(contextFromMetadata([] as unknown as Recording['metadata'])).toEqual({
      contextText: '',
      medicationsText: '',
      allergiesText: '',
      conditionsText: '',
    });
  });

  it('maps a populated patient_context + context string to textarea text', () => {
    const meta: Recording['metadata'] = {
      context: 'Patient follows up on labs.',
      patient_context: {
        medications: ['Lisinopril 10mg', 'Metformin 500mg'],
        allergies: ['Penicillin (rash)'],
        conditions: ['Hypertension', 'Type 2 diabetes'],
      },
    };

    expect(contextFromMetadata(meta)).toEqual({
      contextText: 'Patient follows up on labs.',
      medicationsText: 'Lisinopril 10mg\nMetformin 500mg',
      allergiesText: 'Penicillin (rash)',
      conditionsText: 'Hypertension\nType 2 diabetes',
    });
  });

  it('returns empty strings (not undefined) when sub-lists are missing', () => {
    // patient_context present but with no medications/allergies/conditions keys.
    const meta: Recording['metadata'] = {
      patient_context: {
        medications: [],
        allergies: [],
        conditions: [],
      },
    };
    // Strip the lists to simulate a sparse patient_context (only some keys).
    const sparse = {
      patient_context: { prior_soap_notes: [] },
    } as unknown as Recording['metadata'];

    const result = contextFromMetadata(sparse);
    expect(result.medicationsText).toBe('');
    expect(result.allergiesText).toBe('');
    expect(result.conditionsText).toBe('');
    expect(result.contextText).toBe('');

    // Also verify the typed empty-list path works (sanity).
    const fromTyped = contextFromMetadata(meta);
    expect(fromTyped.medicationsText).toBe('');
  });

  it('treats non-string context as empty', () => {
    // Defensive: a corrupt metadata blob with context as a number.
    const meta = { context: 42 } as unknown as Recording['metadata'];
    expect(contextFromMetadata(meta).contextText).toBe('');
  });

  it('handles empty arrays as empty text', () => {
    const meta: Recording['metadata'] = {
      patient_context: {
        medications: [],
        allergies: [],
        conditions: [],
      },
    };
    expect(contextFromMetadata(meta)).toEqual({
      contextText: '',
      medicationsText: '',
      allergiesText: '',
      conditionsText: '',
    });
  });
});
