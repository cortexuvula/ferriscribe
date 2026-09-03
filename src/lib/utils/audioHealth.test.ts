import { describe, expect, it } from 'vitest';
import { audioHealthAlert } from './audioHealth';
import type { AudioHealthEvent } from '../api/audio';

function event(overrides: Partial<AudioHealthEvent> = {}): AudioHealthEvent {
  return {
    paused: false,
    elapsed_secs: 30,
    duration_secs: 30,
    signal_secs: 25,
    peak: 0.5,
    rms: 0.05,
    total_samples: 480_000,
    has_signal: true,
    secs_since_last_data: 0.2,
    secs_since_last_sound: 0.5,
    stream_error: null,
    ...overrides,
  };
}

describe('audioHealthAlert', () => {
  it('returns null for a healthy capture', () => {
    expect(audioHealthAlert(event())).toBeNull();
  });

  it('returns null when no snapshot has arrived yet', () => {
    expect(audioHealthAlert(null)).toBeNull();
  });

  it('flags an OS stream error as danger, even while paused', () => {
    const alert = audioHealthAlert(
      event({ stream_error: 'device disconnected', paused: true }),
    );
    expect(alert?.level).toBe('danger');
    expect(alert?.message).toContain('device disconnected');
  });

  it('flags a dead stream (no data arriving) as danger', () => {
    const never = audioHealthAlert(
      event({ secs_since_last_data: null, elapsed_secs: 12, has_signal: false }),
    );
    expect(never?.level).toBe('danger');
    expect(never?.message).toContain('not capturing');

    const stopped = audioHealthAlert(event({ secs_since_last_data: 7, has_signal: true }));
    expect(stopped?.level).toBe('danger');
    expect(stopped?.message).toContain('not capturing');
  });

  it('does not flag a dead stream during the startup grace window', () => {
    const early = audioHealthAlert(
      event({ secs_since_last_data: null, elapsed_secs: 2, has_signal: false }),
    );
    expect(early).toBeNull();
  });

  it('warns when data flows but no speech has ever been detected (past grace)', () => {
    // Muted mic: samples (zeros) arrive, none qualify as sound.
    const alert = audioHealthAlert(
      event({ has_signal: false, secs_since_last_sound: null, rms: 0.0, peak: 0.0 }),
    );
    expect(alert?.level).toBe('warning');
    expect(alert?.message).toContain('delivering silence');
  });

  it('does not warn about missing speech within the grace window', () => {
    const early = audioHealthAlert(
      event({ has_signal: false, secs_since_last_sound: null, elapsed_secs: 5 }),
    );
    expect(early).toBeNull();
  });

  it('warns when established signal goes quiet while data still flows', () => {
    const alert = audioHealthAlert(event({ secs_since_last_sound: 14 }));
    expect(alert?.level).toBe('warning');
    expect(alert?.message).toContain('went silent 14s ago');
  });

  it('prefers the dead-stream danger over signal-lost when data also stopped', () => {
    const alert = audioHealthAlert(
      event({ secs_since_last_sound: 30, secs_since_last_data: 12 }),
    );
    expect(alert?.level).toBe('danger');
    expect(alert?.message).toContain('not capturing');
  });

  it('suppresses silence/signal alerts while paused (stream error still shows)', () => {
    expect(audioHealthAlert(event({ paused: true, secs_since_last_data: 60 }))).toBeNull();
    expect(
      audioHealthAlert(event({ paused: true, has_signal: false, secs_since_last_sound: null })),
    ).toBeNull();
    expect(audioHealthAlert(event({ paused: true, secs_since_last_sound: 40 }))).toBeNull();
  });
});
