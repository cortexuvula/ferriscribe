import { writable } from 'svelte/store';
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

function createRsvpStore() {
  const { subscribe, update, set } = writable<RsvpState>(initial);

  function openSoap(rawText: string): void {
    const text = preprocessSoap(rawText ?? '');
    if (!text.trim()) {
      toasts.error('Nothing to read.');
      return;
    }
    const sections = detectSections(text);
    if (sections.length === 0) {
      // No sections detected — skip the picker, read the whole doc.
      update((s) => ({
        ...s,
        reader: { open: true, text, kind: 'soap' },
      }));
      return;
    }
    update((s) => ({
      ...s,
      picker: { open: true, text, sections },
    }));
  }

  function openGeneric(rawText: string, kind: DocKind): void {
    const text = (rawText ?? '').trim();
    if (!text) {
      toasts.error('Nothing to read.');
      return;
    }
    update((s) => ({
      ...s,
      reader: { open: true, text, kind },
    }));
  }

  function startReading(text: string, kind: DocKind): void {
    update((s) => ({
      ...s,
      picker: { open: false, text: '', sections: [] },
      reader: { open: true, text, kind },
    }));
  }

  function closeAll(): void {
    set(initial);
  }

  return { subscribe, openSoap, openGeneric, startReading, closeAll };
}

export const rsvp = createRsvpStore();
