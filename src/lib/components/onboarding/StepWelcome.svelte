<script lang="ts">
  interface Props { onContinue: () => void; }
  const { onContinue }: Props = $props();
</script>

<div class="welcome">
  <div class="welcome-icon" aria-hidden="true">🎙️</div>
  <h1>Welcome to FerriScribe</h1>
  <p class="lead">
    A privacy-first medical scribe. Record a doctor–patient encounter, transcribe
    it locally, and generate SOAP notes, referrals, and letters — without any
    patient data ever leaving your machine.
  </p>
  <ul class="points">
    <li><strong>Local-only.</strong> Your audio, transcripts, and notes stay on this computer.</li>
    <li><strong>No cloud AI.</strong> Works with Ollama, LM Studio, or oMLX running on your machine or LAN.</li>
    <li><strong>Clinical documents.</strong> SOAP notes, referrals, letters, and peer discussions in seconds.</li>
  </ul>
  <button class="btn-primary" onclick={onContinue}>Let's get started →</button>
</div>

<style>
  .welcome { text-align: center; display: flex; flex-direction: column; align-items: center; gap: 14px; }
  .welcome-icon { font-size: 48px; }
  h1 { font-size: 22px; font-weight: 700; margin: 0; color: var(--text-primary); }
  .lead { font-size: 14px; line-height: 1.6; color: var(--text-secondary); margin: 0; max-width: 440px; }
  .points { list-style: none; padding: 0; margin: 4px 0; text-align: left; display: flex; flex-direction: column; gap: 6px; width: 100%; max-width: 440px; }
  .points li { font-size: 13px; color: var(--text-secondary); line-height: 1.5; }
  .points strong { color: var(--text-primary); }
  .btn-primary {
    margin-top: 8px; padding: 10px 24px; background-color: var(--accent, #3b82f6);
    color: white; border: none; border-radius: var(--radius-md, 8px);
    font-size: 14px; font-weight: 600; cursor: pointer;
    transition: background-color 0.15s ease;
  }
  .btn-primary:hover { background-color: var(--accent-hover, #2563eb); }
</style>
