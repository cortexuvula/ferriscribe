import { detectSections, preprocessSoap, type Section } from '../rsvp/engine';
import { toasts } from './toasts.svelte';

export type DocKind = 'soap' | 'referral' | 'letter' | 'synopsis';

export interface RsvpState {
  picker: {
    open: boolean;
    text: string;
    sections: Section[];
  };
  reader: {
    open: boolean;
    text: string;
    kind: DocKind;
  };
}

const initial: RsvpState = {
  picker: { open: false, text: '', sections: [] },
  reader: { open: false, text: '', kind: 'soap' },
};

class RsvpStore {
  state = $state<RsvpState>({
    picker: { open: false, text: '', sections: [] },
    reader: { open: false, text: '', kind: 'soap' },
  });

  openSoap(rawText: string): void {
    const text = preprocessSoap(rawText ?? '');
    if (!text.trim()) {
      toasts.error('Nothing to read.');
      return;
    }
    const sections = detectSections(text);
    if (sections.length === 0) {
      // No sections detected — skip the picker, read the whole doc.
      this.state = {
        ...this.state,
        reader: { open: true, text, kind: 'soap' },
      };
      return;
    }
    this.state = {
      ...this.state,
      picker: { open: true, text, sections },
    };
  }

  openGeneric(rawText: string, kind: DocKind): void {
    const text = (rawText ?? '').trim();
    if (!text) {
      toasts.error('Nothing to read.');
      return;
    }
    this.state = {
      ...this.state,
      reader: { open: true, text, kind },
    };
  }

  startReading(text: string, kind: DocKind): void {
    this.state = {
      ...this.state,
      picker: { open: false, text: '', sections: [] },
      reader: { open: true, text, kind },
    };
  }

  closeAll(): void {
    this.state = { ...initial };
  }
}

export const rsvp = new RsvpStore();
