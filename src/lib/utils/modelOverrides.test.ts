import { describe, expect, it } from 'vitest';
import { staleFeatureModelFields } from './modelOverrides';

describe('staleFeatureModelFields', () => {
  const models = [
    { id: 'qwen3:8b' },
    { id: 'qwen3:1.7b' },
  ];

  it('returns nothing when overrides are unset or still offered', () => {
    expect(
      staleFeatureModelFields(models, { ocr_model: null, translation_model: null })
    ).toEqual([]);
    expect(
      staleFeatureModelFields(models, { ocr_model: 'qwen3:8b', translation_model: 'qwen3:1.7b' })
    ).toEqual([]);
  });

  it('returns fields whose id the provider no longer offers', () => {
    expect(
      staleFeatureModelFields(models, { ocr_model: 'glm-ocr', translation_model: 'qwen3:1.7b' })
    ).toEqual(['ocr_model']);
    expect(
      staleFeatureModelFields(models, { ocr_model: null, translation_model: 'gemma3:4b' })
    ).toEqual(['translation_model']);
    expect(
      staleFeatureModelFields(models, { ocr_model: 'glm-ocr', translation_model: 'gemma3:4b' })
    ).toEqual(['ocr_model', 'translation_model']);
  });

  it('an empty model list makes every set override stale', () => {
    expect(
      staleFeatureModelFields([], { ocr_model: 'glm-ocr', translation_model: null })
    ).toEqual(['ocr_model']);
  });
});
