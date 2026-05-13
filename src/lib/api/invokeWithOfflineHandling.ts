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

// invokeWithOfflineHandling is implemented in Task 11.
