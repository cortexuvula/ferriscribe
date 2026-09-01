import { invoke } from '@tauri-apps/api/core';
import { endpointOfflineStore } from '../stores/endpointOffline.svelte';

/** Discriminant strings emitted by AppError::EndpointOffline. */
export type ServiceKind = 'AiProvider' | 'RemoteStt';
export type OfflineReason = 'ConnectionRefused' | 'Timeout' | 'DnsFailure' | 'TlsFailure';

/** Decoded payload of an `EndpointOffline` rejection from a Tauri invoke. */
export interface EndpointOfflinePayload {
  kind: 'EndpointOffline';
  service: ServiceKind;
  endpoint: string;
  reason: OfflineReason;
  provider_name: string;
  message: string;
}

/** User's choice in the offline dialog. */
export type EndpointOfflineDecision = 'retry' | 'cancel' | 'opened_settings';

/** Sentinel error thrown by `invokeWithOfflineHandling` when the user
 *  dismisses the offline dialog. Callers should `instanceof`-check
 *  and early-return silently — the dialog already explained the situation. */
export class OfflineCancelled extends Error {
  constructor(public reason: 'cancel' | 'opened_settings') {
    super(`User dismissed offline dialog: ${reason}`);
    this.name = 'OfflineCancelled';
    // restore prototype chain for instanceof to work across module boundaries
    Object.setPrototypeOf(this, OfflineCancelled.prototype);
  }
}

/** Type guard for the EndpointOffline rejection shape. */
export function isEndpointOffline(err: unknown): err is EndpointOfflinePayload {
  return (
    typeof err === 'object' &&
    err !== null &&
    (err as { kind?: unknown }).kind === 'EndpointOffline'
  );
}

/** Maximum consecutive offline retries before giving up and throwing the
 *  error. Prevents an endless retry loop when the endpoint is persistently
 *  unreachable (e.g. server down, Tailscale disconnected). */
const MAX_OFFLINE_RETRIES = 3;

/** Wraps Tauri `invoke`. On `EndpointOffline` rejection, opens the
 *  shared dialog and awaits the user's decision:
 *    - Retry      → loops back to re-invoke `cmd` with `args` (up to
 *                   MAX_OFFLINE_RETRIES consecutive failures).
 *    - Cancel     → throws OfflineCancelled('cancel').
 *    - OpenSettings → throws OfflineCancelled('opened_settings').
 *  Any other rejection passes through verbatim.
 *
 *  After MAX_OFFLINE_RETRIES consecutive offline failures, the error is
 *  thrown verbatim instead of reopening the dialog — the endpoint is
 *  clearly persistently unreachable and the user needs to fix it in
 *  Settings rather than clicking Retry endlessly.
 *
 *  Successful retry resumes the original `await` with the new result —
 *  callers don't need to re-trigger their action.
 */
export async function invokeWithOfflineHandling<T>(
  cmd: string,
  args: Record<string, unknown> = {},
): Promise<T> {
  let consecutiveOfflineFailures = 0;
  for (;;) {
    try {
      return await invoke<T>(cmd, args);
    } catch (err) {
      if (!isEndpointOffline(err)) {
        throw err;
      }
      consecutiveOfflineFailures++;
      if (consecutiveOfflineFailures > MAX_OFFLINE_RETRIES) {
        // Give up — the endpoint is persistently unreachable.
        throw err;
      }
      const decision = await endpointOfflineStore.openAndWait(err);
      if (decision === 'retry') {
        continue;
      }
      throw new OfflineCancelled(decision);
    }
  }
}
