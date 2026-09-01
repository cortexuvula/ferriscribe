import type { GenerationProgressStats } from '../types';

export type GeneratingType = 'soap' | 'referral' | 'letter' | 'peer_discussion' | null;

interface GenerationState {
  /** Which document type is currently being generated, or null if idle. */
  generating: GeneratingType;
  /** Live progress text from the backend event. */
  progressStatus: string | null;
  /** Live streaming throughput while a generation is in flight, else null.
   *  Counts and durations only — never content. */
  progress: GenerationProgressStats | null;
  /** Error message from the last generation attempt. */
  error: string | null;
  /** The type that was generating when the last error occurred, for retry. */
  lastFailedType: GeneratingType;
}

class GenerationStore {
  state = $state<GenerationState>({
    generating: null,
    progressStatus: null,
    progress: null,
    error: null,
    lastFailedType: null,
  });

  startGenerating(type: 'soap' | 'referral' | 'letter' | 'peer_discussion') {
    this.state = { ...this.state, generating: type, error: null, progressStatus: null, progress: null };
  }

  setProgress(status: string) {
    this.state = { ...this.state, progressStatus: status };
  }

  /** Set live streaming stats, or pass null to clear them. Cleared on every
   *  lifecycle transition (start/finish/error) so the UI never depends on a
   *  terminal backend event — some flows (e.g. synopsis) never emit one. */
  setProgressStats(p: GenerationProgressStats | null) {
    this.state = { ...this.state, progress: p };
  }

  setError(error: string) {
    // Preserve the generating type as lastFailedType so the error banner
    // can offer a "Retry" affordance for the specific document type.
    this.state = {
      ...this.state,
      generating: null,
      progressStatus: null,
      progress: null,
      error,
      lastFailedType: this.state.generating,
    };
  }

  finish() {
    this.state = { ...this.state, generating: null, progressStatus: null, progress: null, lastFailedType: null };
  }

  clearError() {
    this.state = { ...this.state, error: null, lastFailedType: null };
  }
}

export const generation = new GenerationStore();
