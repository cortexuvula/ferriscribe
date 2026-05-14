/**
 * Clamp a requested sidebar width against three constraints:
 *   - [min, max] absolute bounds
 *   - viewport must allow at least `mainMin` px for the non-sidebar area
 *
 * If even `min` doesn't fit alongside `mainMin`, returns `min` and lets the
 * caller's layout handle the overflow (typically a horizontal scroll).
 */
export function clampSidebarWidth(
  requested: number,
  viewportWidth: number,
  min: number,
  max: number,
  mainMin: number,
): number {
  // First clamp to absolute [min, max].
  let w = Math.max(min, Math.min(max, requested));
  // Then enforce viewport: main area gets at least mainMin px.
  const viewportAllows = viewportWidth - mainMin;
  if (viewportAllows < w) {
    w = Math.max(min, viewportAllows);
  }
  return w;
}
