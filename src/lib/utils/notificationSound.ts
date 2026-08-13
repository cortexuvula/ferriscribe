/**
 * Local notification chime, synthesized with the Web Audio API.
 *
 * Privacy constraints shape this design: no TTS provider, no bundled audio
 * asset, no network — just two short sine tones (A5 → D6) with a quick
 * attack and exponential decay, which reads as a soft "ding-ding" completion
 * cue. Total duration ~0.6 s at a modest peak gain (0.18).
 *
 * Failures (no AudioContext, autoplay policy, detached webview) are swallowed
 * — a notification sound must never break the flow it decorates.
 */

let ctx: AudioContext | null = null;

/** Play the SOAP-note-completed chime. Safe to call repeatedly. */
export function playSoapCompleteChime(): void {
  try {
    ctx ??= new AudioContext();
    if (ctx.state === 'suspended') {
      void ctx.resume();
    }
    const t0 = ctx.currentTime;

    const tone = (freq: number, start: number, duration: number) => {
      const osc = ctx!.createOscillator();
      const gain = ctx!.createGain();
      osc.type = 'sine';
      osc.frequency.value = freq;
      // Quick attack to a modest level, then exponential fade to silence.
      gain.gain.setValueAtTime(0, t0 + start);
      gain.gain.linearRampToValueAtTime(0.18, t0 + start + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.0001, t0 + start + duration);
      osc.connect(gain);
      gain.connect(ctx!.destination);
      osc.start(t0 + start);
      osc.stop(t0 + start + duration);
    };

    tone(880, 0, 0.35); // A5
    tone(1174.66, 0.12, 0.45); // D6 — rising major third, "finished" feel
  } catch {
    // Audio unavailable — the visual toast still notifies the user.
  }
}
