/// Viewport-aware popover placement math (correction batch: Codie S-3 /
/// PARENT-UI-1).
///
/// The parent UI review measured the recording-date calendar popover
/// top-anchored with a fixed max-height clipped inside a 390x320 window
/// (rect bottom 565, not scrollable — a clipping ancestor's
/// overflow:hidden ate it); Codie's 500x430 probe additionally measured it
/// running off the right edge. The correction renders the popover
/// position:fixed (escaping clipping ancestors) and computes its geometry
/// from the AVAILABLE viewport: horizontal edges are clamped, placement
/// flips above the trigger when below lacks room, and when NEITHER side
/// fits the popover clamps to the roomier side with internal scrolling.
/// Never a larger minimum window.

/// Minimum clear space between the popover and every viewport edge.
export const POPOVER_MARGIN = 12;
/// Gap between the trigger's edge and the popover.
export const POPOVER_GAP = 6;
/// Preferred popover width; yields to the viewport width (minus margins).
export const POPOVER_PREFERRED_WIDTH = 322;

export interface TriggerRect {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

export interface PopoverViewport {
  width: number;
  height: number;
}

export interface PopoverPlacement {
  placement: 'below' | 'above';
  /// Viewport-relative fixed-position coordinates (px).
  left: number;
  top: number;
  width: number;
  /// Space available on the chosen side; the popover scrolls internally
  /// (overflow-y) whenever its natural height exceeds this.
  maxHeight: number;
}

/// Compute the popover's fixed-position geometry for a trigger rect, the
/// popover's natural (unclamped) content height, and the viewport.
///
/// Deterministic and pure — the component re-runs it on open, on window
/// resize (including while open), and when the rendered month changes the
/// grid's height; the same math is unit-tested and verified in a real
/// browser against short/narrow/wide viewports.
export function computePopoverPlacement(
  trigger: TriggerRect,
  naturalHeight: number,
  viewport: PopoverViewport,
  preferredWidth: number = POPOVER_PREFERRED_WIDTH,
): PopoverPlacement {
  // Horizontal: prefer right-aligning to the trigger, clamped so the
  // popover never leaves the viewport on either side; the width itself
  // yields when the viewport is narrower than the preferred width.
  const width = Math.max(0, Math.min(preferredWidth, viewport.width - 2 * POPOVER_MARGIN));
  const leftMax = Math.max(POPOVER_MARGIN, viewport.width - POPOVER_MARGIN - width);
  const left = Math.min(Math.max(trigger.right - width, POPOVER_MARGIN), leftMax);

  // Vertical: below when it fits (or is the roomier side), else above.
  // Both spaces are clamped by the margin so the popover never touches
  // the viewport edge, let alone crosses it.
  const spaceBelow = viewport.height - POPOVER_MARGIN - (trigger.bottom + POPOVER_GAP);
  const spaceAbove = trigger.top - POPOVER_GAP - POPOVER_MARGIN;
  const height = Math.max(0, naturalHeight);

  let placement: 'below' | 'above';
  let top: number;
  let maxHeight: number;
  if (height <= spaceBelow || spaceBelow >= spaceAbove) {
    placement = 'below';
    top = trigger.bottom + POPOVER_GAP;
    maxHeight = spaceBelow;
  } else {
    placement = 'above';
    maxHeight = spaceAbove;
    // Fully above when it fits; otherwise fill downward FROM the top
    // margin (the popover scrolls internally for the rest).
    top = Math.max(POPOVER_MARGIN, trigger.top - POPOVER_GAP - Math.min(height, spaceAbove));
  }

  return { placement, left, top, width, maxHeight: Math.max(0, maxHeight) };
}
