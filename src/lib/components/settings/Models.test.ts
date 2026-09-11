// @vitest-environment jsdom
/**
 * Models — the Settings → Models pane. Focused coverage for the per-feature
 * model selects (OCR, translation): the sentinel option, option population
 * from the provider's model list, and the updateField persistence calls.
 */
import { render, screen, fireEvent, waitFor, cleanup, within } from '@testing-library/svelte';
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import Models from './Models.svelte';

const mockState = vi.hoisted(() => ({
  ai_provider: 'ollama',
  ai_model: 'qwen3:8b',
  ocr_model: null as string | null,
  translation_model: null as string | null,
  temperature: 0.2,
  lmstudio_host: 'localhost',
  lmstudio_port: 1234,
  lmstudio_disable_thinking: false,
  ollama_host: 'localhost',
  ollama_port: 11434,
  ollama_disable_thinking: false,
  omlx_host: 'localhost',
  omlx_port: 8000,
  omlx_disable_thinking: false,
}));

const mockUpdateField = vi.hoisted(() => vi.fn(async () => {}));
const mockListModels = vi.hoisted(() => vi.fn(async () => [
  { id: 'qwen3:8b', name: 'qwen3:8b', provider: 'ollama' },
  { id: 'qwen3:1.7b', name: 'qwen3:1.7b', provider: 'ollama' },
]));

vi.mock('../../stores/settings.svelte', () => ({
  settings: { state: mockState, updateField: mockUpdateField },
}));

vi.mock('../../api/chat', () => ({
  listModels: mockListModels,
  setActiveProvider: vi.fn(async () => {}),
}));

vi.mock('../../api/settings', () => ({
  testLmStudioConnection: vi.fn(async () => 'ok'),
  testOllamaConnection: vi.fn(async () => 'ok'),
  testOmlxConnection: vi.fn(async () => 'ok'),
  // Rendered child sections (ProviderServerSection) read/write keychain keys.
  getApiKey: vi.fn(async () => null),
  setApiKey: vi.fn(async () => {}),
}));

vi.mock('../../api/sharing', () => ({
  isPairedWithServer: vi.fn(async () => false),
}));

async function rendered() {
  const view = render(Models);
  // onMount fetches the model list — wait for the options to land.
  await waitFor(() =>
    expect(
      (screen.getByLabelText('Translation Model') as HTMLSelectElement).options.length
    ).toBeGreaterThan(1)
  );
  return view;
}

describe('Models', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockState.ocr_model = null;
    mockState.translation_model = null;
  });
  afterEach(cleanup);

  it('keeps model options collapsed with a summary of configured overrides', async () => {
    mockState.ocr_model = 'qwen3:8b';
    mockState.translation_model = 'qwen3:1.7b';
    const { container } = await rendered();
    const options = screen.getByText('Model options').closest('details')!;
    expect(options.open).toBe(false);
    expect(options.querySelector('summary')?.textContent).toContain('OCR: qwen3:8b');
    expect(options.querySelector('summary')?.textContent).toContain('Translation: qwen3:1.7b');
    expect(container.querySelector('#ai-provider')?.closest('details')).toBeNull();
    expect(container.querySelector('#ai-model')?.closest('details')).toBeNull();
    options.open = true;
    await fireEvent.input(screen.getByLabelText(/Temperature/), { target: { value: '0.7' } });
    expect(mockUpdateField).not.toHaveBeenCalled();
    await fireEvent.change(screen.getByLabelText(/Temperature/));
    expect(mockUpdateField).toHaveBeenCalledWith('temperature', 0.7);
  });

  it('keeps all server controls reachable with the active provider first', async () => {
    const { container } = await rendered();
    const servers = screen.getByText('Server configuration').closest('details')!;
    expect(servers.open).toBe(false);
    servers.open = true;
    const summaries = Array.from(servers.querySelectorAll('.provider-section > summary'));
    expect(summaries[0].textContent).toContain('Ollama Server');
    expect(summaries).toHaveLength(3);
    for (const provider of ['ollama', 'lmstudio', 'omlx']) {
      for (const field of ['host', 'port', 'api-key']) {
        expect(container.querySelector(`#${provider}-${field}`)).toBeTruthy();
      }
    }
    expect(within(servers).getAllByText('Save key')).toHaveLength(3);
    expect(within(servers).getAllByText(/Disable thinking/)).toHaveLength(3);
  });

  it('shows model-list failures outside collapsed server configuration', async () => {
    mockListModels.mockRejectedValueOnce(new Error('Synthetic provider offline'));
    render(Models);
    const error = await screen.findByText('Synthetic provider offline');
    expect(screen.getByText('Server configuration').closest('details')!.open).toBe(false);
    expect(error.closest('details')).toBeNull();
  });

  it('keeps validation failures visible when server configuration is collapsed', async () => {
    const { container } = await rendered();
    const servers = screen.getByText('Server configuration').closest('details')!;
    servers.open = true;
    await fireEvent.change(container.querySelector('#ollama-port')!, { target: { value: '0' } });
    const error = await screen.findByRole('alert');
    servers.open = false;
    await fireEvent(servers, new Event('toggle'));
    await waitFor(() => expect(servers.open).toBe(true));
    expect(error.textContent).toContain('Port must be between 1 and 65535');
  });

  it('renders the translation-model select with the inherit sentinel first', async () => {
    await rendered();
    const select = screen.getByLabelText('Translation Model') as HTMLSelectElement;
    expect(select.value).toBe('');
    expect(select.options[0].textContent).toBe('(use generation model)');
    const values = Array.from(select.options).map((o) => o.value);
    expect(values).toContain('qwen3:8b');
    expect(values).toContain('qwen3:1.7b');
  });

  it('shows the configured override as selected', async () => {
    mockState.translation_model = 'qwen3:1.7b';
    await rendered();
    expect((screen.getByLabelText('Translation Model') as HTMLSelectElement).value).toBe(
      'qwen3:1.7b'
    );
  });

  it('persists a translation-model pick via updateField', async () => {
    await rendered();
    await fireEvent.change(screen.getByLabelText('Translation Model'), {
      target: { value: 'qwen3:1.7b' },
    });
    expect(mockUpdateField).toHaveBeenCalledWith('translation_model', 'qwen3:1.7b');
  });

  it('persists the sentinel choice as null (inherit the generation model)', async () => {
    mockState.translation_model = 'qwen3:1.7b';
    await rendered();
    await fireEvent.change(screen.getByLabelText('Translation Model'), {
      target: { value: '' },
    });
    expect(mockUpdateField).toHaveBeenCalledWith('translation_model', null);
  });
});
