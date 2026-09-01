import { describe, it, expect, beforeEach, vi } from 'vitest';
import { letterWriter } from './letterWriter.svelte';

// ── Mocks ───────────────────────────────────────────────────────────────────

const mockGenerateLetterFromDocument = vi.fn();
const mockOcrDocuments = vi.fn();

vi.mock('../api/generation', () => ({
  generateLetterFromDocument: (...args: unknown[]) => mockGenerateLetterFromDocument(...args),
}));

vi.mock('../api/ocr', () => ({
  ocrDocuments: (...args: unknown[]) => mockOcrDocuments(...args),
}));

// ── Tests ───────────────────────────────────────────────────────────────────

describe('LetterWriterStore', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    letterWriter.handleClearAll();
  });

  it('cannot generate with no source text', () => {
    expect(letterWriter.documentText).toBe('');
    expect(letterWriter.canGenerate).toBe(false);
  });

  it('whitespace-only pasted text does not enable generation', () => {
    letterWriter.pastedText = '   \n\t  ';
    expect(letterWriter.documentText).toBe('');
    expect(letterWriter.canGenerate).toBe(false);
  });

  it('pasted text alone is the source document', async () => {
    letterWriter.pastedText = '  Patient referred for echo.  ';
    mockGenerateLetterFromDocument.mockResolvedValue('Dear Dr. Smith,');

    expect(letterWriter.documentText).toBe('Patient referred for echo.');
    expect(letterWriter.canGenerate).toBe(true);

    await letterWriter.handleGenerate();

    expect(mockGenerateLetterFromDocument).toHaveBeenCalledWith(
      'Patient referred for echo.',
      expect.objectContaining({ tone: 'Formal' }),
    );
    expect(letterWriter.output).toBe('Dear Dr. Smith,');
    expect(letterWriter.generating).toBe(false);
    expect(letterWriter.error).toBeNull();
  });

  it('OCR text alone remains the source document', async () => {
    letterWriter.ocr.handleOcrTextChange('--- report.pdf ---\nLab results attached.');
    mockGenerateLetterFromDocument.mockResolvedValue('letter');

    expect(letterWriter.documentText).toBe('--- report.pdf ---\nLab results attached.');

    await letterWriter.handleGenerate();

    expect(mockGenerateLetterFromDocument).toHaveBeenCalledWith(
      '--- report.pdf ---\nLab results attached.',
      expect.anything(),
    );
  });

  it('joins OCR text and pasted text into one source document', async () => {
    letterWriter.ocr.handleOcrTextChange('--- report.pdf ---\nLab results attached.');
    letterWriter.pastedText = 'Also mention the follow-up plan.';
    mockGenerateLetterFromDocument.mockResolvedValue('letter');

    expect(letterWriter.documentText).toBe(
      '--- report.pdf ---\nLab results attached.\n\nAlso mention the follow-up plan.',
    );

    await letterWriter.handleGenerate();

    expect(mockGenerateLetterFromDocument).toHaveBeenCalledWith(
      '--- report.pdf ---\nLab results attached.\n\nAlso mention the follow-up plan.',
      expect.anything(),
    );
  });

  it('handleClearAll resets the pasted text', () => {
    letterWriter.pastedText = 'some pasted content';
    letterWriter.handleClearAll();
    expect(letterWriter.pastedText).toBe('');
    expect(letterWriter.canGenerate).toBe(false);
  });

  it('captures generation errors', async () => {
    letterWriter.pastedText = 'source';
    mockGenerateLetterFromDocument.mockRejectedValue(new Error('boom'));

    await letterWriter.handleGenerate();

    expect(letterWriter.output).toBe('');
    expect(letterWriter.generating).toBe(false);
    expect(letterWriter.error).toContain('boom');
  });
});
