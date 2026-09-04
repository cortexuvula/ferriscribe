import { describe, it, expect, beforeEach } from 'vitest';
import { generation } from './generation.svelte';

describe('GenerationStore', () => {
  beforeEach(() => {
    generation.clearError();
    generation.finish();
  });

  it('starts in idle state', () => {
    expect(generation.state.generating).toBeNull();
    expect(generation.state.generatingRecordingId).toBeNull();
    expect(generation.state.error).toBeNull();
    expect(generation.state.progressStatus).toBeNull();
    expect(generation.state.lastFailedType).toBeNull();
  });

  it('startGenerating sets the generating type and clears error', () => {
    generation.setError('previous error');
    generation.startGenerating('soap', 'rec-1');
    expect(generation.state.generating).toBe('soap');
    expect(generation.state.generatingRecordingId).toBe('rec-1');
    expect(generation.state.error).toBeNull();
  });

  it('setError captures lastFailedType before nulling generating', () => {
    generation.startGenerating('referral', 'rec-1');
    generation.setError('something went wrong');
    expect(generation.state.generating).toBeNull();
    expect(generation.state.generatingRecordingId).toBeNull();
    expect(generation.state.error).toBe('something went wrong');
    expect(generation.state.lastFailedType).toBe('referral');
  });

  it('finish clears all state', () => {
    generation.startGenerating('letter', 'rec-1');
    generation.setProgress('step 1');
    generation.finish();
    expect(generation.state.generating).toBeNull();
    expect(generation.state.generatingRecordingId).toBeNull();
    expect(generation.state.progressStatus).toBeNull();
    expect(generation.state.lastFailedType).toBeNull();
  });

  it('clearError clears error and lastFailedType', () => {
    generation.startGenerating('soap', 'rec-1');
    generation.setError('oops');
    generation.clearError();
    expect(generation.state.error).toBeNull();
    expect(generation.state.lastFailedType).toBeNull();
  });

  it('setProgress updates progressStatus', () => {
    generation.startGenerating('soap', 'rec-1');
    generation.setProgress('transcribing…');
    expect(generation.state.progressStatus).toBe('transcribing…');
  });

  it('a new generation replaces the tracked recording id', () => {
    generation.startGenerating('soap', 'rec-1');
    generation.finish();
    generation.startGenerating('letter', 'rec-2');
    expect(generation.state.generatingRecordingId).toBe('rec-2');
  });
});
