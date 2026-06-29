import { describe, it, expect, beforeEach, vi } from 'vitest';

const mockList = vi.fn();
const mockUpsert = vi.fn();
const mockDelete = vi.fn();

vi.mock('../api/letterAudiences', () => ({
  listLetterAudiences: (...a: unknown[]) => mockList(...(a as [])),
  upsertLetterAudience: (...a: unknown[]) => mockUpsert(...(a as [unknown])),
  deleteLetterAudience: (...a: unknown[]) => mockDelete(...(a as [string])),
}));

const { letterAudiences } = await import('./letterAudiences.svelte');

describe('LetterAudiencesStore', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('starts empty with no loading or error', () => {
    expect(letterAudiences.audiences).toEqual([]);
    expect(letterAudiences.loading).toBe(false);
    expect(letterAudiences.error).toBeNull();
  });

  it('list populates audiences from the API', async () => {
    const mockData = [
      { id: 'a1', name: 'Cardiology', is_builtin: true, sort_order: 0 },
      { id: 'a2', name: 'ER', is_builtin: false, sort_order: 1 },
    ];
    mockList.mockResolvedValue(mockData);
    await letterAudiences.list();
    expect(letterAudiences.audiences).toEqual(mockData);
    expect(letterAudiences.loading).toBe(false);
    expect(letterAudiences.error).toBeNull();
  });

  it('list sets error on failure', async () => {
    mockList.mockRejectedValue(new Error('Network error'));
    await letterAudiences.list();
    expect(letterAudiences.error).toBe('Network error');
    expect(letterAudiences.loading).toBe(false);
  });

  it('upsert adds a new audience', async () => {
    mockList.mockResolvedValue([]);
    await letterAudiences.list();
    const newAudience = { id: 'a3', name: 'Neurology', is_builtin: false, sort_order: 2 };
    mockUpsert.mockResolvedValue(newAudience);
    const result = await letterAudiences.upsert(newAudience);
    expect(result).toEqual(newAudience);
    expect(letterAudiences.audiences).toHaveLength(1);
    expect(letterAudiences.audiences[0].name).toBe('Neurology');
  });

  it('upsert updates an existing audience', async () => {
    const existing = { id: 'a1', name: 'Cardiology', is_builtin: true, sort_order: 0 };
    mockList.mockResolvedValue([existing]);
    await letterAudiences.list();
    const updated = { ...existing, name: 'Cardiology Dept' };
    mockUpsert.mockResolvedValue(updated);
    await letterAudiences.upsert(updated);
    expect(letterAudiences.audiences).toHaveLength(1);
    expect(letterAudiences.audiences[0].name).toBe('Cardiology Dept');
  });

  it('delete removes an audience', async () => {
    const a1 = { id: 'a1', name: 'A', is_builtin: false, sort_order: 0 };
    const a2 = { id: 'a2', name: 'B', is_builtin: false, sort_order: 1 };
    mockList.mockResolvedValue([a1, a2]);
    await letterAudiences.list();
    mockDelete.mockResolvedValue(undefined);
    await letterAudiences.delete('a1');
    expect(letterAudiences.audiences).toHaveLength(1);
    expect(letterAudiences.audiences[0].id).toBe('a2');
  });
});
