import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock the offline-handling wrapper so we control the invoke result.
const mockInvokeWithOfflineHandling = vi.fn();
vi.mock('./invokeWithOfflineHandling', () => ({
  invokeWithOfflineHandling: (...args: unknown[]) => mockInvokeWithOfflineHandling(...args),
  OfflineCancelled: class OfflineCancelled extends Error {},
}));

import { ocrDocuments } from './ocr';

beforeEach(() => {
  mockInvokeWithOfflineHandling.mockReset();
});

describe('ocr api', () => {
  it('passes filePaths to ocr_documents command', async () => {
    mockInvokeWithOfflineHandling.mockResolvedValue([
      { filename: 'test.pdf', text: 'extracted text', page_count: 1 },
    ]);

    const results = await ocrDocuments(['/path/to/test.pdf']);

    expect(mockInvokeWithOfflineHandling).toHaveBeenCalledWith('ocr_documents', {
      filePaths: ['/path/to/test.pdf'],
    });
    expect(results).toHaveLength(1);
    expect(results[0].filename).toBe('test.pdf');
  });

  it('returns empty array for empty input', async () => {
    mockInvokeWithOfflineHandling.mockResolvedValue([]);
    const results = await ocrDocuments([]);
    expect(results).toEqual([]);
  });

  it('propagates errors from invokeWithOfflineHandling', async () => {
    mockInvokeWithOfflineHandling.mockRejectedValue(new Error('model not loaded'));
    await expect(ocrDocuments(['/path/to/img.png'])).rejects.toThrow('model not loaded');
  });
});
