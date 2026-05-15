import type { VocabularyEntry } from '../api/vocabulary';

export function filterVocabularyEntries(
  entries: VocabularyEntry[],
  searchText: string,
): VocabularyEntry[] {
  if (!searchText.trim()) return entries;
  const q = searchText.toLowerCase();
  return entries.filter(
    (e) =>
      e.find_text.toLowerCase().includes(q) ||
      e.replacement.toLowerCase().includes(q),
  );
}
