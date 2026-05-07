import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { processRecording, cancelPipeline } from './pipeline';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe('pipeline api', () => {
  it('processRecording explicitly null-coalesces all optional fields', async () => {
    await processRecording('rec-1');
    expect(invokeMock).toHaveBeenCalledWith('process_recording', {
      recordingId: 'rec-1',
      context: null,
      template: null,
      patientContext: null,
    });
  });

  it('processRecording forwards context / template / patientContext when provided', async () => {
    const pc = { medications: ['m'], allergies: [], conditions: [] };
    await processRecording('rec-1', 'ctx', 'tpl', pc);
    expect(invokeMock).toHaveBeenCalledWith('process_recording', {
      recordingId: 'rec-1',
      context: 'ctx',
      template: 'tpl',
      patientContext: pc,
    });
  });

  it('cancelPipeline passes recordingId in camelCase', async () => {
    await cancelPipeline('rec-1');
    expect(invokeMock).toHaveBeenCalledWith('cancel_pipeline', { recordingId: 'rec-1' });
  });
});
