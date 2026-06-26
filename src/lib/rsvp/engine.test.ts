import { describe, it, expect } from 'vitest';
import { orpIndex } from './engine';
import { baseDelayMs, delayMs, type Token } from './engine';
import { tokenize, extractIcdCodes } from './engine';
import { preprocessSoap } from './engine';
import { detectSections, type Section } from './engine';

describe('orpIndex', () => {
  it('returns 0 for words of length 1-3', () => {
    expect(orpIndex('I')).toBe(0);
    expect(orpIndex('to')).toBe(0);
    expect(orpIndex('the')).toBe(0);
  });

  it('returns 1 for words of length 4-5', () => {
    expect(orpIndex('hope')).toBe(1);
    expect(orpIndex('heart')).toBe(1);
  });

  it('returns 2 for words of length 6-9', () => {
    expect(orpIndex('doctor')).toBe(2);
    expect(orpIndex('medicine')).toBe(2);
    expect(orpIndex('prescribe')).toBe(2);
  });

  it('returns 3 for words of length 10+', () => {
    expect(orpIndex('hypertension')).toBe(3);
    expect(orpIndex('cardiovascular')).toBe(3);
  });

  it('ignores trailing punctuation', () => {
    expect(orpIndex('hope.')).toBe(1);
    expect(orpIndex('doctor,')).toBe(2);
    expect(orpIndex('well!')).toBe(1);
  });

  it('returns 0 for empty string', () => {
    expect(orpIndex('')).toBe(0);
  });
});

describe('baseDelayMs', () => {
  it('returns 200 for 300 WPM', () => {
    expect(baseDelayMs(300)).toBe(200);
  });
  it('returns 600 for 100 WPM', () => {
    expect(baseDelayMs(100)).toBe(600);
  });
  it('returns 100 for 600 WPM', () => {
    expect(baseDelayMs(600)).toBe(100);
  });
});

describe('delayMs', () => {
  const base = 200;
  it('word: 1.0x', () => {
    const t: Token = { word: 'patient', kind: 'word' };
    expect(delayMs(t, base)).toBe(200);
  });
  it('clause: 1.5x', () => {
    const t: Token = { word: 'patient,', kind: 'clause' };
    expect(delayMs(t, base)).toBe(300);
  });
  it('sentence: 2.5x', () => {
    const t: Token = { word: 'fine.', kind: 'sentence' };
    expect(delayMs(t, base)).toBe(500);
  });
  it('header: 3.0x', () => {
    const t: Token = { word: 'Subjective:', kind: 'header' };
    expect(delayMs(t, base)).toBe(600);
  });
});

describe('tokenize', () => {
  it('classifies plain words', () => {
    const t = tokenize('the patient reports');
    expect(t.map((x) => x.kind)).toEqual(['word', 'word', 'word']);
    expect(t.map((x) => x.word)).toEqual(['the', 'patient', 'reports']);
  });

  it('classifies clause punctuation', () => {
    const t = tokenize('feels well, no complaints');
    expect(t[1]).toEqual({ word: 'well,', kind: 'clause' });
  });

  it('classifies sentence terminators', () => {
    const t = tokenize('He is fine. She is unwell!');
    expect(t[2]).toEqual({ word: 'fine.', kind: 'sentence' });
    expect(t[5]).toEqual({ word: 'unwell!', kind: 'sentence' });
  });

  it('classifies SOAP headers as header kind', () => {
    const t = tokenize('Subjective: the patient reports');
    expect(t[0]).toEqual({ word: 'Subjective:', kind: 'header' });
  });

  it('treats markdown-bold headers as headers', () => {
    const t = tokenize('**Objective:** vitals stable');
    expect(t[0].kind).toBe('header');
  });

  it('does not classify non-SOAP trailing-colon words as header', () => {
    const t = tokenize('Time: 09:30 today');
    expect(t[0].kind).not.toBe('header');
  });

  it('preserves order and word count', () => {
    const t = tokenize('one two three four.');
    expect(t.length).toBe(4);
    expect(t[3]).toEqual({ word: 'four.', kind: 'sentence' });
  });
});

describe('preprocessSoap', () => {
  it('strips ICD-10 codes in parens', () => {
    const out = preprocessSoap('Hypertension (ICD-10: I10)');
    expect(out).not.toMatch(/ICD-10/);
    expect(out).not.toMatch(/I10/);
    expect(out).toContain('Hypertension');
  });

  it('strips ICD-9 codes', () => {
    const out = preprocessSoap('Diabetes (ICD-9: 250.00)');
    expect(out).not.toMatch(/ICD-9/);
    expect(out).not.toMatch(/250/);
  });

  it('strips "Not discussed" lines', () => {
    const out = preprocessSoap(
      'Chief complaint: fatigue\nFamily history: Not discussed\nSocial history: smoker'
    );
    expect(out).not.toMatch(/Not discussed/);
    expect(out).toContain('fatigue');
    expect(out).toContain('smoker');
  });

  it('strips leading bullets and dashes', () => {
    const out = preprocessSoap('- headache\n• nausea\n* vomiting');
    expect(out).not.toMatch(/^[-•*]\s/m);
    expect(out).toContain('headache');
    expect(out).toContain('nausea');
    expect(out).toContain('vomiting');
  });

  it('leaves clean text untouched', () => {
    const clean = 'Patient reports chest pain.';
    expect(preprocessSoap(clean)).toBe(clean);
  });

  it('strips ICD-9 Code: format from SOAP template output', () => {
    const out = preprocessSoap('ICD-9 Code: V70.0\n\nSubjective:\n- Chief complaint: cough');
    expect(out).not.toMatch(/ICD-9/);
    expect(out).not.toMatch(/V70/);
    expect(out).toContain('Subjective');
  });

  it('strips ICD-10 Code: format from SOAP template output', () => {
    const out = preprocessSoap('ICD-10 Code: Z00.00\n\nSubjective:\n- Chief complaint: wellness');
    expect(out).not.toMatch(/ICD-10/);
    expect(out).not.toMatch(/Z00/);
  });
});

describe('extractIcdCodes', () => {
  it('extracts ICD-9 Code: format from SOAP template output', () => {
    const codes = extractIcdCodes('ICD-9 Code: 401.9\n\nSubjective:\n- Chief complaint: headache');
    expect(codes).toContain('ICD-9 Code: 401.9');
  });

  it('extracts ICD-10 Code: format from SOAP template output', () => {
    const codes = extractIcdCodes('ICD-10 Code: I10\n\nSubjective:\n- Chief complaint: hypertension');
    expect(codes).toContain('ICD-10 Code: I10');
  });

  it('extracts inline ICD codes in parentheses', () => {
    const codes = extractIcdCodes('Hypertension (ICD-9: 401.9)');
    expect(codes).toContain('ICD-9: 401.9');
  });

  it('extracts inline ICD-10 codes in brackets', () => {
    const codes = extractIcdCodes('Diabetes [ICD-10: E11.9]');
    expect(codes).toContain('ICD-10: E11.9');
  });

  it('extracts multiple codes', () => {
    const text = 'ICD-9 Code: 401.9\nICD-10 Code: I10\n\nSubjective:\n- hypertension (ICD-9: 401.1)';
    const codes = extractIcdCodes(text);
    expect(codes.length).toBe(3);
  });

  it('returns empty array when no ICD codes present', () => {
    const codes = extractIcdCodes('Subjective:\n- Chief complaint: cough');
    expect(codes).toEqual([]);
  });

  it('extracts BC MSP alpha-suffix codes without dropping the trailing letter', () => {
    // BUG-3 regression: 01A, 10A etc. must keep the trailing alpha, else
    // they fail MSP validation and render a false billing warning.
    const codes = extractIcdCodes('ICD-9 Code: 01A');
    expect(codes).toContain('ICD-9 Code: 01A');
    expect(codes[0]).toMatch(/01A$/);
  });
});

describe('detectSections', () => {
  it('returns [] when no headers present', () => {
    expect(detectSections('just some prose')).toEqual([]);
  });

  it('finds all seven SOAP headers case-insensitively', () => {
    const text = [
      'Subjective:',
      'patient reports fatigue',
      'OBJECTIVE:',
      'BP 140/90',
      'assessment:',
      'hypertension',
      'Plan:',
      'start lisinopril',
    ].join('\n');
    const sections = detectSections(text);
    expect(sections.map((s: Section) => s.name)).toEqual([
      'Subjective',
      'Objective',
      'Assessment',
      'Plan',
    ]);
  });

  it('tolerates markdown bold around the header', () => {
    const sections = detectSections('**Subjective:** patient reports fatigue');
    expect(sections.length).toBe(1);
    expect(sections[0].name).toBe('Subjective');
  });

  it('counts words per section', () => {
    const text = 'Subjective: a b c\nObjective: d e';
    const sections = detectSections(text);
    expect(sections[0].wordCount).toBe(3); // a, b, c (header not counted)
    expect(sections[1].wordCount).toBe(2); // d, e
  });

  it('reports start and end word indices', () => {
    const text = 'Subjective: a b c\nObjective: d e';
    const sections = detectSections(text);
    // tokens: [Subjective:, a, b, c, Objective:, d, e] — indices 0..6
    expect(sections[0].startWordIndex).toBe(0);
    expect(sections[0].endWordIndex).toBe(4); // exclusive
    expect(sections[1].startWordIndex).toBe(4);
    expect(sections[1].endWordIndex).toBe(7);
  });
});
