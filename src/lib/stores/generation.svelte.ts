export type GeneratingType = 'soap' | 'referral' | 'letter' | 'peer_discussion' | null;

interface GenerationState {
  /** Which document type is currently being generated, or null if idle. */
  generating: GeneratingType;
  /** Live progress text from the backend event. */
  progressStatus: string | null;
  /** Error message from the last generation attempt. */
  error: string | null;
  /** The type that was generating when the last error occurred, for retry. */
  lastFailedType: GeneratingType;
}

class GenerationStore {
  state = $state<GenerationState>({
    generating: null,
    progressStatus: null,
    error: null,
    lastFailedType: null,
  });

  startGenerating(type: 'soap' | 'referral' | 'letter' | 'peer_discussion') {
    this.state = { ...this.state, generating: type, error: null, progressStatus: null };
  }

  setProgress(status: string) {
    this.state = { ...this.state, progressStatus: status };
  }

  setError(error: string) {
    // Preserve the generating type as lastFailedType so the error banner
    // can offer a "Retry" affordance for the specific document type.
    this.state = {
      ...this.state,
      generating: null,
      progressStatus: null,
      error,
      lastFailedType: this.state.generating,
    };
  }

  finish() {
    this.state = { ...this.state, generating: null, progressStatus: null, lastFailedType: null };
  }

  clearError() {
    this.state = { ...this.state, error: null, lastFailedType: null };
  }
}

export const generation = new GenerationStore();
