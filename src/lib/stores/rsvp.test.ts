import { describe, it, expect, beforeEach } from 'vitest';
import { rsvp } from './rsvp.svelte';

describe('RsvpStore', () => {
  beforeEach(() => {
    rsvp.closeAll();
  });

  it('starts with picker and reader closed', () => {
    expect(rsvp.state.picker.open).toBe(false);
    expect(rsvp.state.reader.open).toBe(false);
  });

  it('openGeneric opens the reader with the text and kind', () => {
    rsvp.openGeneric('Hello world this is a test', 'letter');
    expect(rsvp.state.reader.open).toBe(true);
    expect(rsvp.state.reader.text).toBe('Hello world this is a test');
    expect(rsvp.state.reader.kind).toBe('letter');
  });

  it('openGeneric with empty text does not open (toast is shown)', () => {
    rsvp.openGeneric('', 'letter');
    expect(rsvp.state.reader.open).toBe(false);
  });

  it('openGeneric with whitespace-only text does not open', () => {
    rsvp.openGeneric('   \n\t  ', 'letter');
    expect(rsvp.state.reader.open).toBe(false);
  });

  it('startReading closes picker and opens reader', () => {
    rsvp.state = {
      ...rsvp.state,
      picker: { open: true, text: 'test', sections: [] },
    };
    rsvp.startReading('read this text', 'soap');
    expect(rsvp.state.picker.open).toBe(false);
    expect(rsvp.state.reader.open).toBe(true);
    expect(rsvp.state.reader.text).toBe('read this text');
  });

  it('closeAll resets to initial state', () => {
    rsvp.openGeneric('some text', 'soap');
    rsvp.closeAll();
    expect(rsvp.state.picker.open).toBe(false);
    expect(rsvp.state.reader.open).toBe(false);
    expect(rsvp.state.picker.sections).toEqual([]);
  });

  it('openSoap with content opens picker or reader', () => {
    const soapText = [
      'Subjective:',
      '- Patient reports headache',
      '',
      'Objective:',
      '- BP 120/80',
      '',
      'Assessment:',
      '- Tension headache',
      '',
      'Plan:',
      '- Ibuprofen 400mg',
    ].join('\n');
    rsvp.openSoap(soapText);
    // With detectable sections, picker opens; without, reader opens directly.
    expect(rsvp.state.picker.open || rsvp.state.reader.open).toBe(true);
  });

  it('openSoap with empty text does not open', () => {
    rsvp.openSoap('');
    expect(rsvp.state.picker.open).toBe(false);
    expect(rsvp.state.reader.open).toBe(false);
  });
});
