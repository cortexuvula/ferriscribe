// Tests for the RUSTSEC-2026-0429 ignore guard. The guard must discriminate
// the four cases the exception documents: accepted residual (exit 0), a NEW
// affected consumer or out-of-scope zip copy (exit 1, fail-closed), malformed
// evidence (exit 1), and the residual being REMOVED upstream (exit 2 — the
// ignore must then be dropped). Plus the enforced review window (exit 3).
// Run: node --test scripts/audit-0429-guard.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { assessResidual } from './audit-0429-guard.mjs';

// Minimal cargo-metadata-shaped fixture builder. `edges` maps a package id to
// the ids of its resolved dependencies (all targets — cargo metadata's
// resolve graph includes platform-specific deps).
function fixture({ zips = [], edges = {} } = {}) {
  const packages = [];
  const nodes = [];
  const idOf = {};
  const add = (name, version) => {
    const id = `registry+https://github.com/rust-lang/crates.io-index#${name}@${version}`;
    idOf[`${name}@${version}`] = id;
    packages.push({ name, version, id });
    return id;
  };
  for (const [name, version] of zips) add(name, version);
  // zip 9.0.5 (patched) plus docx-rs and its consumers are the in-graph actors.
  add('docx-rs', '0.4.22');
  add('medical-export', '0.1.0');
  add('medical-processing', '0.1.0');
  // Default edges mirror the accepted graph: docx-rs -> zip8, export -> docx-rs,
  // processing -> zip9 (zip9 attached below when present).
  const resolvedEdges = {
    'docx-rs@0.4.22': [],
    'medical-export@0.1.0': ['docx-rs@0.4.22'],
    'medical-processing@0.1.0': [],
    ...edges,
  };
  for (const [key, depKeys] of Object.entries(resolvedEdges)) {
    if (!idOf[key]) continue;
    nodes.push({
      id: idOf[key],
      deps: depKeys.filter((k) => idOf[k]).map((k) => ({ name: k.split('@')[0], pkg: idOf[k] })),
    });
  }
  return { packages, resolve: { nodes } };
}

const zip8 = ['zip', '8.6.0'];
const zip9 = ['zip', '9.0.5'];
const zip4 = ['zip', '4.6.1'];
const zip8Dep = { 'docx-rs@0.4.22': ['zip@8.6.0'] };
const zip9Dep = { 'medical-processing@0.1.0': ['zip@9.0.5'] };

const WITHIN_WINDOW = new Date('2026-11-15T00:00:00Z');

test('accepted residual: exactly zip 8.6.0 consumed only by docx-rs 0.4.22', () => {
  const m = fixture({
    zips: [zip8, zip9],
    edges: { ...zip8Dep, ...zip9Dep },
  });
  const v = assessResidual(m, WITHIN_WINDOW);
  assert.equal(v.code, 0, v.message);
  assert.match(v.message, /accepted residual/);
});

test('new affected consumer: a second dependent on zip 8.6.0 fails', () => {
  const m = fixture({
    zips: [zip8, zip9],
    edges: {
      ...zip8Dep,
      'medical-export@0.1.0': ['docx-rs@0.4.22', 'zip@8.6.0'],
      ...zip9Dep,
    },
  });
  const v = assessResidual(m, WITHIN_WINDOW);
  assert.equal(v.code, 1, v.message);
  assert.match(v.message, /medical-export/);
});

test('new affected consumer: zip 4.6.1 (or any non-8.6.0 pre-9 copy) fails', () => {
  for (const bad of [zip4, ['zip', '8.7.0']]) {
    const m = fixture({
      zips: [bad, zip9],
      edges: { 'docx-rs@0.4.22': ['zip@4.6.1'], ...zip9Dep },
    });
    const v = assessResidual(m, WITHIN_WINDOW);
    assert.equal(v.code, 1, v.message);
  }
});

test('different docx-rs version still on zip 8 is outside the exception', () => {
  const m = fixture({ zips: [zip8, zip9], edges: { ...zip8Dep, ...zip9Dep } });
  // docx-rs 0.4.23 appears as an additional dependent of zip 8.6.0.
  m.packages.push({
    name: 'docx-rs',
    version: '0.4.23',
    id: 'registry+https://github.com/rust-lang/crates.io-index#docx-rs@0.4.23',
  });
  m.resolve.nodes.push({
    id: 'registry+https://github.com/rust-lang/crates.io-index#docx-rs@0.4.23',
    deps: [
      {
        name: 'zip',
        pkg: m.packages.find((p) => p.name === 'zip' && p.version === '8.6.0').id,
      },
    ],
  });
  const v = assessResidual(m, WITHIN_WINDOW);
  assert.equal(v.code, 1, v.message);
});

test('malformed evidence: missing resolve graph / packages / null input fails', () => {
  assert.equal(assessResidual(null, WITHIN_WINDOW).code, 1);
  assert.equal(assessResidual({}, WITHIN_WINDOW).code, 1);
  assert.equal(assessResidual({ packages: [] }, WITHIN_WINDOW).code, 1);
  assert.equal(assessResidual({ packages: [], resolve: {} }, WITHIN_WINDOW).code, 1);
  // Unparseable version strings are not pre-9 → they do not grant acceptance;
  // they simply are not the accepted 8.6.0 residual (no zip found → removal).
  const weird = fixture({ zips: [['zip', 'x.y.z']] });
  assert.equal(assessResidual(weird, WITHIN_WINDOW).code, 2);
});

test('removed residual: no zip <9.0.0 anywhere demands dropping the ignore', () => {
  const m = fixture({ zips: [zip9], edges: zip9Dep });
  const v = assessResidual(m, WITHIN_WINDOW);
  assert.equal(v.code, 2, v.message);
  assert.match(v.message, /remove/);
});

test('orphaned zip 8.6.0 (present in lock, no consumer) is not accepted', () => {
  const m = fixture({ zips: [zip8, zip9], edges: zip9Dep });
  const v = assessResidual(m, WITHIN_WINDOW);
  assert.equal(v.code, 1, v.message);
});

test('review window is enforced: past REVIEW_BY the gate fails even for the accepted graph', () => {
  const m = fixture({
    zips: [zip8, zip9],
    edges: { ...zip8Dep, ...zip9Dep },
  });
  const v = assessResidual(m, new Date('2026-12-03T00:00:00Z'));
  assert.equal(v.code, 3, v.message);
  assert.match(v.message, /expired/);
  // ...but the day before the deadline it passes.
  assert.equal(assessResidual(m, new Date('2026-12-02T00:00:00Z')).code, 0);
});
