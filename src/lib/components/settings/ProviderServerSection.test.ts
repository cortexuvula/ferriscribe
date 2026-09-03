// @vitest-environment jsdom
/**
 * ProviderServerSection — render/interaction tests for the shared provider
 * host/port/test-connection/thinking-toggle section (the Settings → Models
 * building block that replaced three hand-maintained copies).
 */
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/svelte';
import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import ProviderServerSection from './ProviderServerSection.svelte';

// Hoisted mutable fixture the mocked settings store reads from — tests
// mutate it directly to exercise the derived state.
const mockState = vi.hoisted(() => ({
  omlx_host: 'localhost',
  omlx_port: 8000,
  omlx_disable_thinking: false,
  allow_public_endpoint: false,
}));

const mockUpdateField = vi.hoisted(() => vi.fn(async () => {}));
const mockReinit = vi.hoisted(() => vi.fn(async () => {}));
const mockGetApiKey = vi.hoisted(() => vi.fn(async (): Promise<string | null> => null));
const mockSetApiKey = vi.hoisted(() => vi.fn(async () => {}));
const mockTestConnection = vi.hoisted(() => vi.fn(async () => '3 models visible'));

vi.mock('../../stores/settings.svelte', () => ({
  settings: { state: mockState, updateField: mockUpdateField },
}));

vi.mock('../../api/chat', () => ({
  reinitProviders: mockReinit,
}));

vi.mock('../../api/settings', () => ({
  getApiKey: mockGetApiKey,
  setApiKey: mockSetApiKey,
}));

function props() {
  return {
    idPrefix: 'omlx',
    title: 'oMLX Server',
    hostField: 'omlx_host' as const,
    portField: 'omlx_port' as const,
    defaultPort: 8000,
    apiKeySlot: 'omlx_api_key',
    testConnection: mockTestConnection,
    thinkingField: 'omlx_disable_thinking' as const,
  };
}

describe('ProviderServerSection', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockState.omlx_host = 'localhost';
    mockState.omlx_port = 8000;
    mockState.omlx_disable_thinking = false;
  });
  afterEach(cleanup);

  it('renders the section with host/port inputs and thinking toggle', () => {
    render(ProviderServerSection, { props: props() });
    expect((screen.getByLabelText('Host') as HTMLInputElement).value).toBe('localhost');
    expect((screen.getByLabelText('Port') as HTMLInputElement).value).toBe('8000');
    expect(screen.getByText('Disable thinking (reasoning models)')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Test Connection' })).toBeTruthy();
  });

  it('hides the thinking toggle when thinkingField is omitted', () => {
    const { thinkingField: _omit, ...rest } = props();
    render(ProviderServerSection, { props: rest });
    expect(screen.queryByText('Disable thinking (reasoning models)')).toBeNull();
  });

  it('tests the connection with the keychain key and shows the success message', async () => {
    render(ProviderServerSection, { props: props() });
    await fireEvent.click(screen.getByRole('button', { name: 'Test Connection' }));
    await waitFor(() => expect(screen.getByText('✓ 3 models visible')).toBeTruthy());
    expect(mockGetApiKey).toHaveBeenCalledWith('omlx_api_key');
    expect(mockTestConnection).toHaveBeenCalledWith('localhost', 8000, null);
  });

  it('renders an optional API key field pre-filled from the keychain', async () => {
    mockGetApiKey.mockResolvedValueOnce('secret-key');
    render(ProviderServerSection, { props: props() });
    await waitFor(() =>
      expect((screen.getByLabelText('API key (optional)') as HTMLInputElement).value).toBe('secret-key'),
    );
  });

  it('saves the typed key to the keychain slot and re-inits providers', async () => {
    render(ProviderServerSection, { props: props() });
    const keyField = screen.getByLabelText('API key (optional)');
    await fireEvent.input(keyField, { target: { value: 'new-key' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Save key' }));
    await waitFor(() => expect(screen.getByText('✓ Key saved.')).toBeTruthy());
    expect(mockSetApiKey).toHaveBeenCalledWith('omlx_api_key', 'new-key');
    expect(mockReinit).toHaveBeenCalled();
  });

  it('tests with the key typed in the field over the stored one', async () => {
    render(ProviderServerSection, { props: props() });
    const keyField = screen.getByLabelText('API key (optional)');
    await fireEvent.input(keyField, { target: { value: 'typed-key' } });
    mockGetApiKey.mockClear(); // drop the onMount prefill call
    await fireEvent.click(screen.getByRole('button', { name: 'Test Connection' }));
    await waitFor(() => expect(mockTestConnection).toHaveBeenCalledWith('localhost', 8000, 'typed-key'));
    expect(mockGetApiKey).not.toHaveBeenCalled();
  });

  it('shows the error message when the test rejects', async () => {
    mockTestConnection.mockRejectedValueOnce(new Error('oMLX at http://x:8000 is offline'));
    render(ProviderServerSection, { props: props() });
    await fireEvent.click(screen.getByRole('button', { name: 'Test Connection' }));
    await waitFor(() => expect(screen.getByText(/offline/)).toBeTruthy());
  });

  it('persists host edits and re-inits providers', async () => {
    render(ProviderServerSection, { props: props() });
    const host = screen.getByLabelText('Host');
    await fireEvent.change(host, { target: { value: '192.168.1.50' } });
    expect(mockUpdateField).toHaveBeenCalledWith('omlx_host', '192.168.1.50');
    expect(mockReinit).toHaveBeenCalled();
  });

  it('rejects out-of-range ports without persisting, and says why', async () => {
    render(ProviderServerSection, { props: props() });
    const port = screen.getByLabelText('Port');
    await fireEvent.change(port, { target: { value: '99999' } });
    expect(mockUpdateField).not.toHaveBeenCalled();
    expect(mockReinit).not.toHaveBeenCalled();
    // The failure is visible AND the field reverts to the persisted value
    // (an unsaved invalid port must not linger in the input).
    expect(screen.getByRole('alert').textContent).toBe('Port must be between 1 and 65535.');
    expect((port as HTMLInputElement).value).toBe('8000');
  });

  it('warns on public-internet hosts', () => {
    mockState.omlx_host = 'omlx.example.com';
    render(ProviderServerSection, { props: props() });
    expect(screen.getByText(/public-internet address/)).toBeTruthy();
  });
});
