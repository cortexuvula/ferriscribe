import { describe, it, expect, vi, beforeEach } from 'vitest';

const mockInvokeWithOfflineHandling = vi.fn();
vi.mock('./invokeWithOfflineHandling', () => ({
  invokeWithOfflineHandling: (...args: unknown[]) => mockInvokeWithOfflineHandling(...args),
  OfflineCancelled: class OfflineCancelled extends Error {},
}));

const mockToasts = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
  add: vi.fn(),
}));
vi.mock('../stores/toasts.svelte', () => ({ toasts: mockToasts }));

import {
  captureRegionOcr,
  captureOutcomeMessage,
  toastOcrFailure,
  toastOcrOutcome,
} from './screenshotOcr';

beforeEach(() => {
  mockInvokeWithOfflineHandling.mockReset();
  mockToasts.success.mockReset();
  mockToasts.error.mockReset();
  mockToasts.add.mockReset();
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

  describe('toastOcrOutcome — the single toast mapping every trigger site shares', () => {
    it('success-toasts a copy', () => {
      toastOcrOutcome({ status: 'copied', chars: 7 });
      expect(mockToasts.success).toHaveBeenCalledWith(
        'OCR text copied to clipboard (7 characters)'
      );
      expect(mockToasts.add).not.toHaveBeenCalled();
    });

    it('auto-dismiss-notices cancellations and empty extractions', () => {
      toastOcrOutcome({ status: 'cancelled', chars: 0 });
      expect(mockToasts.add).toHaveBeenCalledWith({
        message: 'Region selection cancelled',
        type: 'success',
        autoDismiss: true,
      });
      mockToasts.add.mockClear();
      toastOcrOutcome({ status: 'empty', chars: 0 });
      expect(mockToasts.add).toHaveBeenCalledWith({
        message: 'No text found in the selected region',
        type: 'success',
        autoDismiss: true,
      });
      expect(mockToasts.success).not.toHaveBeenCalled();
    });

    it('stays silent for the typed concurrent-trigger no-op', () => {
      toastOcrOutcome({ status: 'in_progress', chars: 0 });
      expect(mockToasts.success).not.toHaveBeenCalled();
      expect(mockToasts.add).not.toHaveBeenCalled();
      expect(mockToasts.error).not.toHaveBeenCalled();
    });
  });

  it('toastOcrFailure error-toasts the formatted message', () => {
    toastOcrFailure(new Error('provider offline'));
    expect(mockToasts.error).toHaveBeenCalledWith(
      'Screenshot OCR failed: provider offline'
    );
  });
});
