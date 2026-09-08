import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import {
  deleteLetterAudience,
  listLetterAudiences,
  upsertLetterAudience,
} from './letterAudiences';
import type { LetterAudience } from '../types/letterAudience';

const invokeMock = vi.mocked(invoke);

const audience: LetterAudience = {
  id: 'insurer',
  name: 'Insurer',
  system_prompt: 'Write for an insurer.',
  user_template: null,
  is_builtin: false,
  created_at: '2026-09-08T00:00:00Z',
  updated_at: '2026-09-08T00:00:00Z',
};

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe('letterAudiences api', () => {
  it('list invokes with no args', async () => {
    await listLetterAudiences();
    expect(invokeMock).toHaveBeenCalledWith('list_letter_audiences');
  });

  it('upsert passes the whole audience object', async () => {
    await upsertLetterAudience(audience);
    expect(invokeMock).toHaveBeenCalledWith('upsert_letter_audience', {
      audience,
    });
  });

  it('delete passes the id', async () => {
    await deleteLetterAudience('insurer');
    expect(invokeMock).toHaveBeenCalledWith('delete_letter_audience', {
      id: 'insurer',
    });
  });
});
