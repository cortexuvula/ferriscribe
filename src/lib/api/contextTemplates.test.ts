import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import {
  listContextTemplates,
  upsertContextTemplate,
  renameContextTemplate,
  deleteContextTemplate,
  importContextTemplatesJson,
  exportContextTemplatesJson,
} from './contextTemplates';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe('contextTemplates api', () => {
  it('listContextTemplates invokes list_context_templates with no args', async () => {
    await listContextTemplates();
    expect(invokeMock).toHaveBeenCalledWith('list_context_templates');
  });

  it('upsertContextTemplate passes name + body', async () => {
    await upsertContextTemplate('Visit', 'preamble');
    expect(invokeMock).toHaveBeenCalledWith('upsert_context_template', { name: 'Visit', body: 'preamble' });
  });

  it('renameContextTemplate passes oldName + newName', async () => {
    await renameContextTemplate('Old', 'New');
    expect(invokeMock).toHaveBeenCalledWith('rename_context_template', { oldName: 'Old', newName: 'New' });
  });

  it('deleteContextTemplate passes name', async () => {
    await deleteContextTemplate('Visit');
    expect(invokeMock).toHaveBeenCalledWith('delete_context_template', { name: 'Visit' });
  });

  it('importContextTemplatesJson / exportContextTemplatesJson pass filePath', async () => {
    await importContextTemplatesJson('/tmp/in.json');
    expect(invokeMock).toHaveBeenLastCalledWith('import_context_templates_json', { filePath: '/tmp/in.json' });
    await exportContextTemplatesJson('/tmp/out.json');
    expect(invokeMock).toHaveBeenLastCalledWith('export_context_templates_json', { filePath: '/tmp/out.json' });
  });
});
