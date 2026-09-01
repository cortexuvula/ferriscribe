/**
 * Frontend cache of the BC MSP ICD-9 code set, loaded once via the
 * `get_icd9_code_set` Tauri command. Used by ICD chip validation to
 * flag codes the LLM emitted that are not on the MSP-accepted list.
 *
 * `codeSet` is `null` until the load resolves. Consumers treat `null`
 * as "can't validate" and render chips neutrally (no false warnings).
 *
 * `descriptions` (the official code → description map, via
 * `get_icd9_descriptions`) is a separate best-effort load: it only
 * supplies the billing-code list's explaining titles, so its failure
 * never trips the validation-retry notice — rows fall back to the
 * note's own description text.
 */
import { invoke } from '@tauri-apps/api/core';

class Icd9Store {
  /** The full set of MSP-accepted codes, or null while loading. */
  codeSet = $state<Set<string> | null>(null);
  /** True once the load has resolved (success or failure). */
  loaded = $state(false);
  /** True if the load failed; chips render neutrally when set. */
  loadError = $state(false);
  /** Official code → description map for list titles, or null while
   *  loading / after a failure (cosmetic fallback only). */
  descriptions = $state<Map<string, string> | null>(null);
  private loadPromise: Promise<void> | null = null;
  private descPromise: Promise<void> | null = null;

  /**
   * Triggers a load if one hasn't started (or the last one failed).
   * Safe to call from multiple components — returns a shared promise so
   * the command fires once per pending attempt. After a failure, the
   * guard is cleared so a caller can retry.
   */
  load(): Promise<void> {
    if (this.loadPromise) return this.loadPromise;
    this.loadPromise = this.doLoad();
    return this.loadPromise;
  }

  /** Retries the load after a failure (clears the guard first). */
  retry(): Promise<void> {
    this.loadPromise = null;
    return this.load();
  }

  /** Retries the description load after a failure (clears the guard). */
  retryDescriptions(): Promise<void> {
    this.descPromise = null;
    return this.loadDescriptions();
  }

  /**
   * Loads the official MSP code → description map (best-effort, deduped
   * like `load`). Failures leave `descriptions` null — the billing-code
   * list then shows only descriptions found in the note text. The guard
   * is cleared on failure so a later call can retry.
   */
  loadDescriptions(): Promise<void> {
    if (this.descPromise) return this.descPromise;
    this.descPromise = this.doLoadDescriptions();
    return this.descPromise;
  }

  private async doLoadDescriptions(): Promise<void> {
    try {
      const obj = await invoke<Record<string, string>>('get_icd9_descriptions');
      this.descriptions = new Map(Object.entries(obj));
    } catch (err) {
      console.error('Failed to load ICD-9 descriptions:', err);
      // Cosmetic only — code validation (codeSet) is unaffected. Leave
      // descriptions null and clear the guard so a caller can retry.
      this.descPromise = null;
    }
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
      // so chips render neutrally rather than as false warnings. Clear the
      // guard so a caller (or the retry notice) can re-attempt.
      this.loaded = true;
      this.loadError = true;
      this.loadPromise = null;
    }
  }
}

export const icd9 = new Icd9Store();
