/**
 * Per-feature model overrides (`ocr_model`, `translation_model`) and the
 * staleness rule they share: an id stored from one provider's list that the
 * newly-active provider doesn't offer would silently 404 at request time,
 * so a provider switch must clear it.
 */
import type { AppConfig } from '../types';

/** AppConfig fields holding optional per-feature model overrides. */
export const FEATURE_MODEL_FIELDS = ['ocr_model', 'translation_model'] as const;

export type FeatureModelField = (typeof FEATURE_MODEL_FIELDS)[number];

/**
 * Feature-model fields whose stored id is not offered by `models` — the
 * fields a provider switch must clear (empty result = nothing stale).
 * An unset (null) field is never stale.
 */
export function staleFeatureModelFields(
  models: { id: string }[],
  state: Pick<AppConfig, FeatureModelField>
): FeatureModelField[] {
  return FEATURE_MODEL_FIELDS.filter((field) => {
    const value = state[field];
    return value !== null && !models.some((m) => m.id === value);
  });
}
