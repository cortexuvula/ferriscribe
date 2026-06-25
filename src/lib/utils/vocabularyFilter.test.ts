import { describe, it, expect } from 'vitest';
import { filterVocabularyEntries } from './vocabularyFilter';
import type { VocabularyEntry } from '../api/vocabulary';

function makeEntry(
  find_text: string,
  replacement: string,
  id = find_text,
): VocabularyEntry {
  return {
    id,
    find_text,
    replacement,
    category: 'general',
    case_sensitive: false,
    priority: 0,
    enabled: true,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  };
}

const entries = [
  makeEntry('htn', 'hypertension'),
  makeEntry('t2dm', 'type 2 diabetes mellitus'),
  makeEntry('ckd', 'chronic kidney disease'),
];

describe('filterVocabularyEntries', () => {
  it('returns all entries when search is empty', () => {
    expect(filterVocabularyEntries(entries, '')).toHaveLength(3);
  });

  it('returns all entries when search is whitespace only', () => {
    expect(filterVocabularyEntries(entries, '   ')).toHaveLength(3);
  });

  it('matches on find_text (case-insensitive)', () => {
    const out = filterVocabularyEntries(entries, 'HTN');
    expect(out).toHaveLength(1);
    expect(out[0].find_text).toBe('htn');
  });

  it('matches on replacement text', () => {
    const out = filterVocabularyEntries(entries, 'diabetes');
    expect(out).toHaveLength(1);
    expect(out[0].replacement).toContain('diabetes');
  });

  it('matches on partial substring', () => {
    const out = filterVocabularyEntries(entries, 'kidney');
    expect(out).toHaveLength(1);
    expect(out[0].replacement).toContain('kidney');
  });

  it('returns empty for no match', () => {
    expect(filterVocabularyEntries(entries, 'asthma')).toHaveLength(0);
  });

  it('matches multiple entries sharing a substring', () => {
    const shared = [
      makeEntry('t2dm', 'type 2 diabetes'),
      makeEntry('t1dm', 'type 1 diabetes'),
    ];
    const out = filterVocabularyEntries(shared, 'type');
    expect(out).toHaveLength(2);
  });
});
