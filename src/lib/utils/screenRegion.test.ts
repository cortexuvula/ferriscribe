import { describe, it, expect } from 'vitest';
import { normalizeDrag, isRealSelection } from './screenRegion';

describe('screenRegion geometry', () => {
  it('normalizes a down-right drag', () => {
    expect(normalizeDrag({ x: 10, y: 10 }, { x: 110, y: 60 })).toEqual({
      x: 10,
      y: 10,
      width: 100,
      height: 50,
    });
  });

  it('normalizes an up-left drag (negative deltas)', () => {
    expect(normalizeDrag({ x: 110, y: 60 }, { x: 10, y: 10 })).toEqual({
      x: 10,
      y: 10,
      width: 100,
      height: 50,
    });
  });

  it('handles coordinate-zero drags and degenerate points', () => {
    expect(normalizeDrag({ x: 0, y: 0 }, { x: 800, y: 600 })).toEqual({
      x: 0,
      y: 0,
      width: 800,
      height: 600,
    });
    expect(normalizeDrag({ x: 5, y: 5 }, { x: 5, y: 5 })).toEqual({
      x: 5,
      y: 5,
      width: 0,
      height: 0,
    });
  });

  it('treats sub-4px drags as accidental clicks', () => {
    expect(isRealSelection({ x: 0, y: 0, width: 3, height: 300 })).toBe(false);
    expect(isRealSelection({ x: 0, y: 0, width: 300, height: 3 })).toBe(false);
    expect(isRealSelection({ x: 0, y: 0, width: 4, height: 4 })).toBe(true);
    expect(isRealSelection({ x: 0, y: 0, width: 0, height: 0 })).toBe(false);
  });
});
