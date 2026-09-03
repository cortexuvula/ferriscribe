<script lang="ts">
  import { settings } from '../../../stores/settings.svelte';
  import { reinitProviders } from '../../../api/chat';
</script>

<details class="advanced-section">
  <summary>Advanced</summary>
  <div class="advanced-content">
    <label class="form-row">
      <input
        type="checkbox"
        checked={settings.state.allow_public_endpoint}
        onchange={async (e) => {
          const input = e.target as HTMLInputElement;
          if (
            input.checked &&
            !confirm(
              'Allow public AI / STT endpoints?\n\n' +
                'By default FerriScribe blocks public-internet AI or STT hosts to keep PHI on-device. ' +
                'Enabling this lets transcripts and AI requests leave this machine to servers on the public internet.\n\n' +
                "Continue only if your clinic's privacy policy permits it.",
            )
          ) {
            // Refused: restore the checkbox — the setting was never written.
            input.checked = false;
            return;
          }
          await settings.updateField('allow_public_endpoint', input.checked);
          try { await reinitProviders(); } catch (err) { console.error('Failed to reinit providers after allow_public change:', err); }
        }}
      />
      <span>
        Allow public AI / STT endpoints
        <p class="hint">
          By default, FerriScribe blocks public-internet AI or STT hosts to keep
          PHI on-device. Enable this only if you understand that data may leave
          your machine.
        </p>
      </span>
    </label>

    <label class="form-row">
      <input
        type="checkbox"
        checked={settings.state.capture_for_training ?? false}
        onchange={(e) => settings.updateField('capture_for_training', (e.target as HTMLInputElement).checked)}
      />
      <span>
        Capture generations for training corpus
        <p class="hint">
          Records every SOAP generation and your edited final version into a
          local-device pool (encrypted whenever your database is encrypted).
          Useful for fine-tuning a model on your own dictation style later.
          Data stays on this device — nothing is sent anywhere.
        </p>
      </span>
    </label>
  </div>
</details>

<style>
  .advanced-section summary {
    cursor: pointer;
    font-weight: 600;
    margin-top: 16px;
  }

  .advanced-content {
    margin-top: 8px;
    padding-left: 16px;
  }

  .form-row {
    display: flex;
    gap: 10px;
    align-items: flex-start;
  }

  .hint {
    color: var(--text-muted);
    font-size: 0.8rem;
    margin: 4px 0 0 0;
  }
</style>
