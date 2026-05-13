import { writable, type Readable } from 'svelte/store';
import type {
  EndpointOfflineDecision,
  EndpointOfflinePayload,
} from '../api/invokeWithOfflineHandling';

interface OpenState {
  payload: EndpointOfflinePayload;
  resolve: (decision: EndpointOfflineDecision) => void;
}

function createStore() {
  const state = writable<OpenState | null>(null);
  let current: OpenState | null = null;
  state.subscribe((s) => (current = s));

  return {
    subscribe: state.subscribe as Readable<OpenState | null>['subscribe'],

    /** Opens the dialog with `payload`; resolves when the user picks an
     *  action (retry / cancel / opened_settings). If `openAndWait` is
     *  called while another dialog is pending, the prior promise resolves
     *  with the new decision — matches the "modal at most one" rule. */
    openAndWait(payload: EndpointOfflinePayload): Promise<EndpointOfflineDecision> {
      return new Promise((resolve) => {
        const priorResolve = current?.resolve;
        state.set({
          payload,
          resolve: (decision) => {
            priorResolve?.(decision);
            resolve(decision);
          },
        });
      });
    },

    /** Internal: dialog component calls this when the user picks an action. */
    _resolve(decision: EndpointOfflineDecision): void {
      const s = current;
      if (s) {
        state.set(null);
        s.resolve(decision);
      }
    },

    /** Imperative close without resolving — used in teardown / tests. */
    close(): void {
      state.set(null);
    },
  };
}

export const endpointOfflineStore = createStore();
export type EndpointOfflineStore = typeof endpointOfflineStore;
