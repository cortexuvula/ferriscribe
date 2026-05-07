import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { exportPdf, exportDocx, exportFhir } from './export';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue([]);
});

describe('export api', () => {
  it('exportPdf passes recordingId + exportType', async () => {
    await exportPdf('rec-1', 'soap');
    expect(invokeMock).toHaveBeenCalledWith('export_pdf', { recordingId: 'rec-1', exportType: 'soap' });
  });

  it('exportDocx supports referral / letter export types', async () => {
    await exportDocx('rec-1', 'referral');
    expect(invokeMock).toHaveBeenLastCalledWith('export_docx', { recordingId: 'rec-1', exportType: 'referral' });
    await exportDocx('rec-2', 'letter');
    expect(invokeMock).toHaveBeenLastCalledWith('export_docx', { recordingId: 'rec-2', exportType: 'letter' });
  });

  it('exportFhir passes only recordingId', async () => {
    await exportFhir('rec-1');
    expect(invokeMock).toHaveBeenCalledWith('export_fhir', { recordingId: 'rec-1' });
  });
});
