import { describe, expect, it } from 'vitest';
import { isProtected, parseTimeOfDay, type BackupStatus } from './backup';

function status(over: Partial<BackupStatus>): BackupStatus {
  return {
    everRan: true,
    lastRunAt: new Date().toISOString(),
    snapshotId: 'snap-x',
    drillPassed: true,
    stale: false,
    failure: null,
    pushedTo: '/Volumes/BK',
    wrappingKeyPresent: true,
    scheduleInstalled: true,
    toolCopyOk: true,
    scheduleSupported: true,
    destinationKind: 'folder',
    destinationPresent: true,
    destinationMissing: false,
    ...over,
  };
}

describe('isProtected', () => {
  it('is true for a configured, drilled, fresh backup', () => {
    expect(isProtected(status({}))).toBe(true);
  });
  it('local-only is never protected even when drills pass', () => {
    expect(isProtected(status({ destinationKind: 'local-only' }))).toBe(false);
  });
  it('missing escrow key is not protected', () => {
    expect(isProtected(status({ wrappingKeyPresent: false }))).toBe(false);
  });
  it('a failed drill is not protected', () => {
    expect(isProtected(status({ drillPassed: false }))).toBe(false);
  });
  it('a stale run is not protected', () => {
    expect(isProtected(status({ stale: true }))).toBe(false);
  });
});

describe('parseTimeOfDay', () => {
  it('parses HH:MM', () => {
    expect(parseTimeOfDay('03:30')).toEqual([3, 30]);
    expect(parseTimeOfDay('23:59')).toEqual([23, 59]);
  });
  it('rejects out-of-range and garbage', () => {
    expect(parseTimeOfDay('24:00')).toBeNull();
    expect(parseTimeOfDay('03:60')).toBeNull();
    expect(parseTimeOfDay('around 3')).toBeNull();
    expect(parseTimeOfDay('')).toBeNull();
  });
});
