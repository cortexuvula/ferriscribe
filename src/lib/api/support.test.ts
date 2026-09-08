import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { exportSupportBundle } from './support';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe('support api', () => {
  it('exportSupportBundle invokes with the chosen path', async () => {
    await exportSupportBundle('/tmp/support.zip');
    expect(invokeMock).toHaveBeenCalledWith('export_support_bundle', {
      filePath: '/tmp/support.zip',
    });
  });

  it('propagates export failures', async () => {
    invokeMock.mockRejectedValue(new Error('bundle failed'));
    await expect(exportSupportBundle('/tmp/x')).rejects.toThrow('bundle failed');
  });
});
