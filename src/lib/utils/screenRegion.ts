/** Geometry helpers for the screen-region overlay (X11/Windows capture
 *  path). Pure and unit-tested — mirrors the Rust-side clamping rules. */

export interface DragPoint {
  x: number;
  y: number;
}

export interface CssRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Normalize a start/end drag pair into a positive-dimension rect (CSS px
 *  relative to the overlay viewport, which spans the whole virtual desktop). */
export function normalizeDrag(start: DragPoint, end: DragPoint): CssRect {
  return {
    x: Math.min(start.x, end.x),
    y: Math.min(start.y, end.y),
    width: Math.abs(end.x - start.x),
    height: Math.abs(end.y - start.y),
  };
}

/** Minimum selectable extent in CSS px — anything smaller is an accidental
 *  click, treated as a cancel by the caller. */
export const MIN_SELECT_PX = 4;

/** Whether a normalized rect is a real selection (not an accidental click). */
export function isRealSelection(rect: CssRect): boolean {
  return rect.width >= MIN_SELECT_PX && rect.height >= MIN_SELECT_PX;
}
