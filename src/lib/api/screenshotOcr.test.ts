import { describe, it, expect, vi, beforeEach } from 'vitest';

const mockInvokeWithOfflineHandling = vi.fn();
vi.mock('./invokeWithOfflineHandling', () => ({
  invokeWithOfflineHandling: (...args: unknown[]) => mockInvokeWithOfflineHandling(...args),
  OfflineCancelled: class OfflineCancelled extends Error {},
}));

import { captureRegionOcr, captureOutcomeMessage } from './screenshotOcr';

beforeEach(() => {
  mockInvokeWithOfflineHandling.mockReset();
});

describe('screenshotOcr api', () => {
  it('invokes capture_region_ocr with no arguments', async () => {
    mockInvokeWithOfflineHandling.mockResolvedValue({ status: 'copied', chars: 120 });
    const outcome = await captureRegionOcr();
    expect(mockInvokeWithOfflineHandling).toHaveBeenCalledWith('capture_region_ocr', {});
    expect(outcome.status).toBe('copied');
    expect(outcome.chars).toBe(120);
  });

  it('propagates errors (model not configured, provider offline)', async () => {
    mockInvokeWithOfflineHandling.mockRejectedValue(new Error('No OCR model configured'));
    await expect(captureRegionOcr()).rejects.toThrow('No OCR model configured');
  });

  it('maps outcomes to content-free toast messages', () => {
    expect(captureOutcomeMessage({ status: 'copied', chars: 42 })).toBe(
      'OCR text copied to clipboard (42 characters)'
    );
    expect(captureOutcomeMessage({ status: 'cancelled', chars: 0 })).toBe(
      'Region selection cancelled'
    );
    expect(captureOutcomeMessage({ status: 'empty', chars: 0 })).toBe(
      'No text found in the selected region'
    );
  });
});
