import { describe, it, expect, beforeEach, vi } from 'vitest';
import { useOcr } from './useOcr.svelte';
import type { OcrFileStatus } from '../components/OcrDropZone.svelte';

// ── Mocks ───────────────────────────────────────────────────────────────────

const mockOcrDocuments = vi.fn();

vi.mock('../api/ocr', () => ({
  ocrDocuments: (...args: unknown[]) => mockOcrDocuments(...args),
}));

vi.mock('../api/invokeWithOfflineHandling', () => ({
  OfflineCancelled: class OfflineCancelled extends Error {},
}));

// ── Fixtures ────────────────────────────────────────────────────────────────

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (v: T) => void;
  reject: (e: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function result(filename: string, text: string, page_count = 1) {
  return { filename, text, page_count };
}

/** Let the async handler run past its `await ocrDocuments(...)` continuation. */
async function flush() {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.spyOn(console, 'error').mockImplementation(() => {});
});

// ── Tests ───────────────────────────────────────────────────────────────────

describe('useOcr', () => {
  it('adds loading chips immediately and maps results back by filename', async () => {
    mockOcrDocuments.mockResolvedValue([
      result('a.pdf', 'text A', 3),
      result('missing.pdf', 'never matched'),
    ]);
    const ocr = useOcr();

    ocr.handleOcrFilesSelected(['/tmp/a.pdf']);
    expect(ocr.ocrFiles).toHaveLength(1);
    expect(ocr.ocrFiles[0].status).toBe('loading');
    expect(ocr.ocrLoading).toBe(true);

    await flush();
    expect(ocr.ocrLoading).toBe(false);
    const a = ocr.ocrFiles.find((f) => f.filename === 'a.pdf');
    expect(a?.status).toBe('done');
    expect(a?.pageCount).toBe(3);
    expect(ocr.ocrText).toBe('--- a.pdf ---\ntext A');
  });

  it('marks chips without a matching result as error', async () => {
    mockOcrDocuments.mockResolvedValue([result('a.pdf', 'text A')]);
    const ocr = useOcr();

    ocr.handleOcrFilesSelected(['/tmp/a.pdf', '/tmp/b.pdf']);
    await flush();

    const b = ocr.ocrFiles.find((f) => f.filename === 'b.pdf');
    expect(b?.status).toBe('error');
  });

  it('deduplicates repeated paths in one selection', async () => {
    mockOcrDocuments.mockResolvedValue([result('a.pdf', 'x')]);
    const ocr = useOcr();

    ocr.handleOcrFilesSelected(['/tmp/a.pdf', '/tmp/a.pdf']);
    expect(ocr.ocrFiles).toHaveLength(1);
  });

  it('PHI guard: a stale success after clearOcr() cannot write state', async () => {
    const d = deferred<Awaited<ReturnType<typeof mockOcrDocuments>>>();
    mockOcrDocuments.mockReturnValue(d.promise);
    const ocr = useOcr();

    ocr.handleOcrFilesSelected(['/tmp/a.pdf']);
    expect(ocr.ocrFiles).toHaveLength(1);

    // Patient switch: clears state and bumps the batch token.
    ocr.clearOcr();
    expect(ocr.ocrFiles).toHaveLength(0);

    // The in-flight OCR for the PREVIOUS patient resolves late — its
    // callback must be invalidated, not merged into the new context.
    d.resolve([result('a.pdf', 'PATIENT A PHI')]);
    await flush();

    expect(ocr.ocrFiles).toHaveLength(0);
    expect(ocr.ocrText).toBe('');
    expect(ocr.ocrLoading).toBe(false);
  });

  it('PHI guard: a stale failure after clearOcr() cannot write state', async () => {
    const d = deferred<Awaited<ReturnType<typeof mockOcrDocuments>>>();
    mockOcrDocuments.mockReturnValue(d.promise);
    const ocr = useOcr();

    ocr.handleOcrFilesSelected(['/tmp/a.pdf']);
    ocr.clearOcr();
    d.reject(new Error('boom'));
    await flush();

    expect(ocr.ocrFiles).toHaveLength(0);
  });

  it('concurrent drops share the batch token and merge their chips', async () => {
    const d1 = deferred<Awaited<ReturnType<typeof mockOcrDocuments>>>();
    const d2 = deferred<Awaited<ReturnType<typeof mockOcrDocuments>>>();
    mockOcrDocuments.mockReturnValueOnce(d1.promise).mockReturnValueOnce(d2.promise);
    const ocr = useOcr();

    ocr.handleOcrFilesSelected(['/tmp/a.pdf']);
    ocr.handleOcrFilesSelected(['/tmp/b.pdf']); // second drop while first in flight
    expect(ocr.ocrFiles).toHaveLength(2);
    expect(ocr.ocrFiles.every((f: OcrFileStatus) => f.status === 'loading')).toBe(true);

    d1.resolve([result('a.pdf', 'text A')]);
    await flush();
    expect(ocr.ocrFiles.find((f) => f.filename === 'a.pdf')?.status).toBe('done');
    expect(ocr.ocrFiles.find((f) => f.filename === 'b.pdf')?.status).toBe('loading');
    expect(ocr.ocrLoading).toBe(true); // only clears when no batch remains

    d2.resolve([result('b.pdf', 'text B')]);
    await flush();
    expect(ocr.ocrFiles.every((f) => f.status === 'done')).toBe(true);
    expect(ocr.ocrLoading).toBe(false);
    expect(ocr.ocrText).toBe('--- a.pdf ---\ntext A\n\n--- b.pdf ---\ntext B');
  });

  it('a non-offline rejection marks only that batch’s chips as error', async () => {
    mockOcrDocuments.mockRejectedValue(new Error('provider down'));
    const ocr = useOcr();

    ocr.handleOcrFilesSelected(['/tmp/a.pdf']);
    await flush();

    expect(ocr.ocrFiles[0].status).toBe('error');
    expect(ocr.ocrLoading).toBe(false);
    expect(console.error).toHaveBeenCalled();
  });

  it('an OfflineCancelled rejection is not logged as an error', async () => {
    const { OfflineCancelled } = await import('../api/invokeWithOfflineHandling');
    mockOcrDocuments.mockRejectedValue(new OfflineCancelled());
    const ocr = useOcr();

    ocr.handleOcrFilesSelected(['/tmp/a.pdf']);
    await flush();

    expect(ocr.ocrFiles[0].status).toBe('error');
    expect(console.error).not.toHaveBeenCalled();
  });

  it('manual text edits override the derived text until a file is removed', async () => {
    mockOcrDocuments.mockResolvedValue([result('a.pdf', 'text A'), result('b.pdf', 'text B')]);
    const ocr = useOcr();
    ocr.handleOcrFilesSelected(['/tmp/a.pdf', '/tmp/b.pdf']);
    await flush();

    ocr.handleOcrTextChange('edited text');
    expect(ocr.ocrTextDisplay).toBe('edited text');

    // Removing a file discards the override and rebuilds from remaining chips.
    const bId = ocr.ocrFiles.find((f) => f.filename === 'b.pdf')!.id;
    ocr.handleRemoveOcrFile(bId);
    expect(ocr.ocrTextDisplay).toBe('--- a.pdf ---\ntext A');
  });

  it('clearOcr resets files, text, and the override', async () => {
    mockOcrDocuments.mockResolvedValue([result('a.pdf', 'text A')]);
    const ocr = useOcr();
    ocr.handleOcrFilesSelected(['/tmp/a.pdf']);
    await flush();
    ocr.handleOcrTextChange('edited');

    ocr.clearOcr();
    expect(ocr.ocrFiles).toHaveLength(0);
    expect(ocr.ocrText).toBe('');
    expect(ocr.ocrTextDisplay).toBe('');
  });
});
