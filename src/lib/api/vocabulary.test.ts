import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import {
  listVocabularyEntries,
  addVocabularyEntry,
  updateVocabularyEntry,
  deleteVocabularyEntry,
  deleteAllVocabularyEntries,
  getVocabularyCount,
  importVocabularyJson,
  exportVocabularyJson,
  testVocabularyCorrection,
} from './vocabulary';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe('vocabulary api', () => {
  it('listVocabularyEntries null-coalesces category', async () => {
    await listVocabularyEntries();
    expect(invokeMock).toHaveBeenCalledWith('list_vocabulary_entries', { category: null });
    invokeMock.mockReset();
    await listVocabularyEntries('drugs');
    expect(invokeMock).toHaveBeenCalledWith('list_vocabulary_entries', { category: 'drugs' });
  });

  it('addVocabularyEntry null-coalesces all optional fields', async () => {
    await addVocabularyEntry('asprin', 'aspirin');
    expect(invokeMock).toHaveBeenCalledWith('add_vocabulary_entry', {
      findText: 'asprin',
      replacement: 'aspirin',
      category: null,
      caseSensitive: null,
      priority: null,
      enabled: null,
    });
  });

  it('addVocabularyEntry forwards every optional field when provided', async () => {
    await addVocabularyEntry('asprin', 'aspirin', 'drugs', true, 5, false);
    expect(invokeMock).toHaveBeenCalledWith('add_vocabulary_entry', {
      findText: 'asprin',
      replacement: 'aspirin',
      category: 'drugs',
      caseSensitive: true,
      priority: 5,
      enabled: false,
    });
  });

  it('updateVocabularyEntry passes id + fields with null defaults', async () => {
    await updateVocabularyEntry('id-1', 'a', 'b');
    expect(invokeMock).toHaveBeenCalledWith('update_vocabulary_entry', {
      id: 'id-1',
      findText: 'a',
      replacement: 'b',
      category: null,
      caseSensitive: null,
      priority: null,
      enabled: null,
    });
  });

  it('deleteVocabularyEntry / deleteAllVocabularyEntries / getVocabularyCount have the right shapes', async () => {
    await deleteVocabularyEntry('id-1');
    expect(invokeMock).toHaveBeenLastCalledWith('delete_vocabulary_entry', { id: 'id-1' });
    await deleteAllVocabularyEntries();
    expect(invokeMock).toHaveBeenLastCalledWith('delete_all_vocabulary_entries');
    await getVocabularyCount();
    expect(invokeMock).toHaveBeenLastCalledWith('get_vocabulary_count');
  });

  it('importVocabularyJson / exportVocabularyJson pass filePath in camelCase', async () => {
    await importVocabularyJson('/tmp/in.json');
    expect(invokeMock).toHaveBeenLastCalledWith('import_vocabulary_json', { filePath: '/tmp/in.json' });
    await exportVocabularyJson('/tmp/out.json');
    expect(invokeMock).toHaveBeenLastCalledWith('export_vocabulary_json', { filePath: '/tmp/out.json' });
  });

  it('testVocabularyCorrection passes text', async () => {
    await testVocabularyCorrection('the patient takes asprin');
    expect(invokeMock).toHaveBeenCalledWith('test_vocabulary_correction', { text: 'the patient takes asprin' });
  });
});
