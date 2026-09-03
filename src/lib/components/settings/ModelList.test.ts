// @vitest-environment jsdom
/**
 * ModelList — render/interaction tests for the shared downloadable-model
 * row list (extracted from the WhisperLocalSection/DiarizationModelsSection
 * copies). Verifies the per-row states (downloaded / downloading / ready)
 * and the isDeleteDisabled wiring the whisper list uses to protect the
 * active model.
 */
import { render, screen, fireEvent, cleanup } from '@testing-library/svelte';
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import ModelList from './ModelList.svelte';
import type { DownloadableModel } from '../../api/models';

function model(overrides: Partial<DownloadableModel>): DownloadableModel {
  return {
    id: 'test-model',
    filename: 'test-model.bin',
    size_bytes: 1_600_000_000,
    download_url: 'https://example.com/test-model.bin',
    description: 'A test model',
    downloaded: false,
    ...overrides,
  };
}

const formatBytes = (bytes: number): string => `${Math.round(bytes / 1_048_576)} MB`;

describe('ModelList', () => {
  const onDownload = vi.fn();
  const onDelete = vi.fn();

  beforeEach(() => {
    onDownload.mockClear();
    onDelete.mockClear();
  });
  afterEach(cleanup);

  it('renders a row per model with size and description', () => {
    render(ModelList, {
      props: {
        models: [model({ id: 'large', description: 'Fast large model', size_bytes: 1_048_576 })],
        downloadingModels: new Set<string>(),
        downloadProgress: {},
        onDownload,
        onDelete,
        formatBytes,
      },
    });
    expect(screen.getByText('large')).toBeTruthy();
    expect(screen.getByText('Fast large model')).toBeTruthy();
    expect(screen.getByText('1 MB')).toBeTruthy();
  });

  it('shows a Downloaded badge and Delete button for downloaded models', () => {
    render(ModelList, {
      props: {
        models: [model({ downloaded: true })],
        downloadingModels: new Set<string>(),
        downloadProgress: {},
        onDownload,
        onDelete,
        formatBytes,
      },
    });
    expect(screen.getByText('Downloaded')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Download' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Delete' }));
    expect(onDelete).toHaveBeenCalledWith('test-model');
  });

  it('shows live download percentage while a model is in flight', () => {
    render(ModelList, {
      props: {
        models: [model({})],
        downloadingModels: new Set(['test-model']),
        downloadProgress: { 'test-model': { downloaded: 800_000_000, total: 1_600_000_000 } },
        onDownload,
        onDelete,
        formatBytes,
      },
    });
    expect(screen.getByText('50%')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Download' })).toBeNull();
  });

  it('disables sibling Download buttons while one download is in flight', () => {
    render(ModelList, {
      props: {
        models: [model({ id: 'a' }), model({ id: 'b' })],
        downloadingModels: new Set(['a']),
        downloadProgress: {},
        onDownload,
        onDelete,
        formatBytes,
      },
    });
    const downloadB = screen.getByRole('button', { name: 'Download' });
    expect(downloadB).toBeTruthy();
    expect((downloadB as HTMLButtonElement).disabled).toBe(true); // serialized behind 'a'
  });

  it('disables Delete for models blocked by isDeleteDisabled', () => {
    render(ModelList, {
      props: {
        models: [model({ id: 'active', downloaded: true }), model({ id: 'other', downloaded: true })],
        downloadingModels: new Set<string>(),
        downloadProgress: {},
        onDownload,
        onDelete,
        formatBytes,
        isDeleteDisabled: (m) => m.id === 'active',
        deleteDisabledTitle: 'Cannot delete the active model',
      },
    });
    const activeDelete = screen.getAllByRole('button', { name: 'Delete' })[0];
    expect((activeDelete as HTMLButtonElement).disabled).toBe(true);
    expect(activeDelete.getAttribute('title')).toBe('Cannot delete the active model');
    const otherDelete = screen.getAllByRole('button', { name: 'Delete' })[1];
    expect((otherDelete as HTMLButtonElement).disabled).toBe(false);
  });

  it('calls onDownload when Download is clicked', () => {
    render(ModelList, {
      props: {
        models: [model({})],
        downloadingModels: new Set<string>(),
        downloadProgress: {},
        onDownload,
        onDelete,
        formatBytes,
      },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Download' }));
    expect(onDownload).toHaveBeenCalledWith('test-model');
  });
});
