import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import {
  generateSoap,
  generateReferral,
  generateLetter,
  generateSynopsis,
  generatePeerDiscussion,
} from './generation';

const invokeMock = vi.mocked(invoke);

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue('');
});

describe('generation api', () => {
  it('generateSoap explicitly null-coalesces optional fields', async () => {
    // Defensive null pattern: Tauri's invoke serializes undefined as omitted,
    // so the wrappers explicitly pass null to map to Rust Option::None
    // regardless of how serde defaults are configured.
    await generateSoap('rec-1');
    expect(invokeMock).toHaveBeenCalledWith('generate_soap', {
      recordingId: 'rec-1',
      template: null,
      context: null,
      patientContext: null,
    });
  });

  it('generateSoap forwards template / context / patientContext when provided', async () => {
    const pc = { medications: [], allergies: [], conditions: [] };
    await generateSoap('rec-1', 'tpl', 'ctx', pc);
    expect(invokeMock).toHaveBeenCalledWith('generate_soap', {
      recordingId: 'rec-1',
      template: 'tpl',
      context: 'ctx',
      patientContext: pc,
    });
  });

  it('generateReferral null-coalesces recipientType + urgency + context', async () => {
    await generateReferral('rec-1');
    expect(invokeMock).toHaveBeenCalledWith('generate_referral', {
      recordingId: 'rec-1',
      recipientType: null,
      urgency: null,
      context: null,
    });
  });

  it('generateReferral forwards context when provided', async () => {
    await generateReferral('rec-1', 'cardiology', 'routine', 'extra context');
    expect(invokeMock).toHaveBeenCalledWith('generate_referral', {
      recordingId: 'rec-1',
      recipientType: 'cardiology',
      urgency: 'routine',
      context: 'extra context',
    });
  });

  it('generateLetter null-coalesces letterType + audienceId + context', async () => {
    await generateLetter('rec-1', 'discharge');
    expect(invokeMock).toHaveBeenCalledWith('generate_letter', {
      recordingId: 'rec-1',
      letterType: 'discharge',
      audienceId: null,
      context: null,
    });
    invokeMock.mockReset();
    invokeMock.mockResolvedValue('');
    await generateLetter('rec-2');
    expect(invokeMock).toHaveBeenCalledWith('generate_letter', {
      recordingId: 'rec-2',
      letterType: null,
      audienceId: null,
      context: null,
    });
  });

  it('generateLetter forwards context when provided', async () => {
    await generateLetter('rec-1', 'discharge', 'aud-1', 'extra context');
    expect(invokeMock).toHaveBeenCalledWith('generate_letter', {
      recordingId: 'rec-1',
      letterType: 'discharge',
      audienceId: 'aud-1',
      context: 'extra context',
    });
  });

  it('generatePeerDiscussion null-coalesces context when omitted', async () => {
    await generatePeerDiscussion('rec-1', 'Dr. Smith', 'Cardiology', 'follow-up');
    expect(invokeMock).toHaveBeenCalledWith('generate_peer_discussion', {
      recordingId: 'rec-1',
      physicianName: 'Dr. Smith',
      specialty: 'Cardiology',
      reason: 'follow-up',
      context: null,
    });
  });

  it('generatePeerDiscussion forwards context when provided', async () => {
    await generatePeerDiscussion('rec-1', 'Dr. Smith', 'Cardiology', 'follow-up', 'extra context');
    expect(invokeMock).toHaveBeenCalledWith('generate_peer_discussion', {
      recordingId: 'rec-1',
      physicianName: 'Dr. Smith',
      specialty: 'Cardiology',
      reason: 'follow-up',
      context: 'extra context',
    });
  });

  it('generateSynopsis passes only recordingId', async () => {
    await generateSynopsis('rec-1');
    expect(invokeMock).toHaveBeenCalledWith('generate_synopsis', { recordingId: 'rec-1' });
  });
});
