import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { getDefaultPrompt } from './prompts';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue('');
});

describe('prompts api', () => {
  it('getDefaultPrompt passes docType in camelCase for each known doc kind', async () => {
    for (const k of ['soap', 'referral', 'letter', 'synopsis'] as const) {
      invokeMock.mockReset();
      await getDefaultPrompt(k);
      expect(invokeMock).toHaveBeenCalledWith('get_default_prompt', { docType: k });
    }
  });
});
