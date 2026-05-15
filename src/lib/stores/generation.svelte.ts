export type GeneratingType = 'soap' | 'referral' | 'letter' | null;

interface GenerationState {
  /** Which document type is currently being generated, or null if idle. */
  generating: GeneratingType;
  /** Live progress text from the backend event. */
  progressStatus: string | null;
  /** Error message from the last generation attempt. */
  error: string | null;
}

class GenerationStore {
  state = $state<GenerationState>({
    generating: null,
    progressStatus: null,
    error: null,
  });

  startGenerating(type: 'soap' | 'referral' | 'letter') {
    this.state = { ...this.state, generating: type, error: null, progressStatus: null };
  }

  setProgress(status: string) {
    this.state = { ...this.state, progressStatus: status };
  }

  setError(error: string) {
    this.state = { ...this.state, generating: null, progressStatus: null, error };
  }

  finish() {
    this.state = { ...this.state, generating: null, progressStatus: null };
  }

  clearError() {
    this.state = { ...this.state, error: null };
  }
}

export const generation = new GenerationStore();
