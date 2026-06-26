/**
 * Frontend cache of the BC MSP ICD-9 code set, loaded once via the
 * `get_icd9_code_set` Tauri command. Used by ICD chip validation to
 * flag codes the LLM emitted that are not on the MSP-accepted list.
 *
 * `codeSet` is `null` until the load resolves. Consumers treat `null`
 * as "can't validate" and render chips neutrally (no false warnings).
 */
import { invoke } from '@tauri-apps/api/core';

class Icd9Store {
  /** The full set of MSP-accepted codes, or null while loading. */
  codeSet = $state<Set<string> | null>(null);
  /** True once the load has resolved (success or failure). */
  loaded = $state(false);
  /** True if the load failed; chips render neutrally when set. */
  loadError = $state(false);
  private loadPromise: Promise<void> | null = null;

  /**
   * Triggers a load if one hasn't started. Safe to call from multiple
   * components — returns a shared promise so the command fires once.
   */
  load(): Promise<void> {
    if (this.loadPromise) return this.loadPromise;
    this.loadPromise = this.doLoad();
    return this.loadPromise;
  }

  private async doLoad(): Promise<void> {
    try {
      const codes = await invoke<string[]>('get_icd9_code_set');
      this.codeSet = new Set(codes);
      this.loaded = true;
      this.loadError = false;
    } catch (err) {
      console.error('Failed to load ICD-9 code set:', err);
      // Leave codeSet null — validation treats null as "can't validate",
      // so chips render neutrally rather than as false warnings.
      this.loaded = true;
      this.loadError = true;
    }
  }
}

export const icd9 = new Icd9Store();
