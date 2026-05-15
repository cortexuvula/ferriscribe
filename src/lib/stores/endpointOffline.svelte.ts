import type {
  EndpointOfflineDecision,
  EndpointOfflinePayload,
} from '../api/invokeWithOfflineHandling';

interface OpenState {
  payload: EndpointOfflinePayload;
  resolve: (decision: EndpointOfflineDecision) => void;
}

class EndpointOfflineStoreClass {
  state = $state<OpenState | null>(null);

  /** Opens the dialog with `payload`; resolves when the user picks an
   *  action (retry / cancel / opened_settings). If `openAndWait` is
   *  called while another dialog is pending, the prior promise resolves
   *  with the new decision — matches the "modal at most one" rule. */
  openAndWait(payload: EndpointOfflinePayload): Promise<EndpointOfflineDecision> {
    return new Promise((resolve) => {
      const priorResolve = this.state?.resolve;
      this.state = {
        payload,
        resolve: (decision) => {
          priorResolve?.(decision);
          resolve(decision);
        },
      };
    });
  }

  /** Internal: dialog component calls this when the user picks an action. */
  _resolve(decision: EndpointOfflineDecision): void {
    const s = this.state;
    if (s) {
      this.state = null;
      s.resolve(decision);
    }
  }

  /** Imperative close without resolving — used in teardown / tests. */
  close(): void {
    this.state = null;
  }
}

export const endpointOfflineStore = new EndpointOfflineStoreClass();
export type EndpointOfflineStore = typeof endpointOfflineStore;
