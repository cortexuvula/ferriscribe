import { formatError } from '../types/errors';

export type ConnectionTestStatus = 'idle' | 'testing' | 'success' | 'error';

/**
 * The `'idle' | 'testing' | 'success' | 'error'` state machine behind every
 * "Test Connection" button (provider server sections in Settings → Models,
 * the remote-STT section, onboarding's provider step). The caller supplies
 * the test itself; this composable owns the status/message transitions,
 * the formatError mapping, and a re-entrancy guard while a test is in
 * flight.
 *
 * Returns getters (`status`, `message`) so consumers stay reactive.
 */
export function useTestConnection() {
  let status = $state<ConnectionTestStatus>('idle');
  let message = $state('');

  /** Run a test that resolves to a success message. Re-entrant calls
   * while testing are ignored. */
  async function run(test: () => Promise<string>): Promise<void> {
    if (status === 'testing') return;
    status = 'testing';
    message = '';
    try {
      message = await test();
      status = 'success';
    } catch (e) {
      message = formatError(e) || 'Connection failed';
      status = 'error';
    }
  }

  /** Clear the result — call when the underlying config (host/port)
   * changes so a stale ✓/✗ doesn't outlive the edit. */
  function reset(): void {
    status = 'idle';
    message = '';
  }

  return {
    get status() {
      return status;
    },
    get message() {
      return message;
    },
    run,
    reset,
  };
}
