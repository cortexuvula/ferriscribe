// U2 regression tests: prereleases must never displace stable releases
// from the update channel, and drafts must never be pruned.
// Run: node --test scripts/prune-old-releases.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { planPrune } from './prune-old-releases.mjs';

const rel = (tagName, isDraft = false) => ({ tagName, isDraft });

test('prereleases cannot displace the stable release (the U2 bug)', () => {
  // One stable release + enough newer prereleases to exceed any mixed
  // quota — the old code deleted the stable release here.
  const inventory = [
    rel('v1.0.0'), // the only stable — the update channel
    rel('v1.1.0-beta.1'),
    rel('v1.1.0-beta.2'),
    rel('v1.1.0-beta.3'),
    rel('v1.1.0-beta.4'),
    rel('v1.1.0-beta.5'),
    rel('v1.1.0-beta.6'),
  ];
  const { delete_ } = planPrune(inventory, { stableKeep: 5, preKeep: 5 });
  assert.ok(!delete_.includes('v1.0.0'), `stable v1.0.0 must survive, got deletions: ${delete_}`);
  // Only the oldest prerelease beyond its own pool's quota goes.
  assert.deepEqual(delete_, ['v1.1.0-beta.1']);
});

test('stable pool keeps its own N newest by semver', () => {
  const { keep, delete_ } = planPrune(
    ['v0.9.0', 'v1.0.0', 'v1.1.0', 'v1.2.0', 'v1.3.0', 'v1.4.0', 'v1.5.0'].map((t) => rel(t)),
    { stableKeep: 5, preKeep: 5 },
  );
  assert.ok(['v1.1.0', 'v1.2.0', 'v1.3.0', 'v1.4.0', 'v1.5.0'].every((t) => keep.includes(t)));
  assert.deepEqual(delete_, ['v0.9.0', 'v1.0.0']);
});

test('drafts are never deleted, even beyond quota', () => {
  const { delete_ } = planPrune(
    [
      rel('v1.0.0'),
      rel('v1.1.0'),
      rel('v1.2.0'),
      rel('v1.3.0'),
      rel('v1.4.0'),
      rel('v1.5.0'),
      rel('v1.6.0', true), // in-flight draft from the Release workflow
    ],
    { stableKeep: 5, preKeep: 5 },
  );
  assert.ok(!delete_.includes('v1.6.0'), 'drafts belong to the release workflow, not the pruner');
});

test('non-version tags are ignored entirely', () => {
  const { delete_ } = planPrune([rel('whisper-server-v3'), rel('v1.0.0')], {
    stableKeep: 5,
    preKeep: 5,
  });
  assert.deepEqual(delete_, []);
});

test('semver precedence: v1.0.0 ranks above v1.0.0-beta.9', () => {
  const { stable, prerelease } = planPrune([rel('v1.0.0-beta.9'), rel('v1.0.0')], {
    stableKeep: 5,
    preKeep: 5,
  });
  assert.deepEqual(stable, ['v1.0.0']);
  assert.deepEqual(prerelease, ['v1.0.0-beta.9']);
});
