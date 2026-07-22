import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { ocrDocuments } from './ocr';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
});

describe('ocr api', () => {
  it('passes filePaths to ocr_documents command', async () => {
    invokeMock.mockResolvedValue([
      { filename: 'test.pdf', text: 'extracted text', page_count: 1 },
    ]);

    const results = await ocrDocuments(['/path/to/test.pdf']);

    expect(invokeMock).toHaveBeenCalledWith('ocr_documents', {
      filePaths: ['/path/to/test.pdf'],
    });
    expect(results).toHaveLength(1);
    expect(results[0].filename).toBe('test.pdf');
  });

  it('returns empty array for empty input', async () => {
    invokeMock.mockResolvedValue([]);
    const results = await ocrDocuments([]);
    expect(results).toEqual([]);
  });
});
