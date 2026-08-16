import type { Recording } from '../types';

type RecordingMetadata = Recording['metadata'];

const DOC_TYPES = ['soap', 'referral', 'letter', 'synopsis', 'peer_discussion'] as const;

/**
 * Mirror of the Rust `latest_tokens_per_second` helper
 * (crates/core/src/types/recording.rs): the `tokens_per_second` of the
 * stat with the newest `generated_at` across doc types, or null when no
 * stats are recorded. Entries with a non-numeric throughput or an
 * unparseable `generated_at` are skipped, matching Rust's strict
 * deserialization behavior. The data arrives as freeform JSON from the
 * backend, so each entry is treated as unknown-shaped rather than
 * trusting the declared `GenerationStat` type.
 */
export function latestTokensPerSecond(metadata: RecordingMetadata): number | null {
  if (!metadata) return null;
  const stats = metadata.generation_stats;
  if (!stats) return null;

  let bestTps: number | null = null;
  let bestAt = Number.NEGATIVE_INFINITY;
  for (const docType of DOC_TYPES) {
    const stat = stats[docType] as
      | { tokens_per_second?: unknown; generated_at?: unknown }
      | undefined;
    if (!stat) continue;
    if (typeof stat.tokens_per_second !== 'number') continue;
    if (typeof stat.generated_at !== 'string') continue;
    const parsed = Date.parse(stat.generated_at);
    if (Number.isNaN(parsed)) continue;
    if (bestTps === null || parsed >= bestAt) {
      bestTps = stat.tokens_per_second;
      bestAt = parsed;
    }
  }
  return bestTps;
}
