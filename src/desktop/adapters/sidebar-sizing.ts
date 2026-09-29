/** The left sidebar's widths: it opens at the default, and closes below the minimum. */
export const LEFT_MIN_WIDTH = 200
export const LEFT_DEFAULT_WIDTH = 250
export const LEFT_MAX_WIDTH = 450

/** The right panel's widths. It is never narrower than its minimum: it closes instead. */
export const RIGHT_MIN_WIDTH = 200
export const RIGHT_DEFAULT_WIDTH = 400

/** The workspace between them, while it is expanded. */
export const WORKSPACE_MIN_WIDTH = 350

/**
 * Fits requested pixel widths into the window. Space is given in this order:
 *
 * 1. the workspace keeps its minimum (unless it has been snapped closed);
 * 2. an open right panel keeps its minimum — it has precedence over the left;
 * 3. an open left sidebar takes what it asked for, narrowing to what is left
 *    and closing if that is under its minimum;
 * 4. the right panel grows toward what it asked for with whatever remains.
 *
 * So opening the right panel in a narrow window narrows or closes the left,
 * and the right never shrinks to a sliver: below its minimum it closes.
 */
export function fitSidebarWidths(
  width: number,
  requestedLeft = LEFT_DEFAULT_WIDTH,
  requestedRight = RIGHT_DEFAULT_WIDTH,
  workspaceCollapsed = false,
) {
  const wantLeft =
    requestedLeft > 0
      ? Math.min(LEFT_MAX_WIDTH, Math.max(LEFT_MIN_WIDTH, requestedLeft))
      : 0
  const sidebars = Math.max(0, width - (workspaceCollapsed ? 0 : WORKSPACE_MIN_WIDTH))
  const rightFloor =
    requestedRight > 0 && sidebars >= RIGHT_MIN_WIDTH ? RIGHT_MIN_WIDTH : 0

  const leftRoom = Math.min(wantLeft, sidebars - rightFloor)
  const left = leftRoom >= LEFT_MIN_WIDTH ? leftRoom : 0

  if (workspaceCollapsed) return { left, center: 0, right: width - left }
  const right = rightFloor
    ? Math.min(Math.max(RIGHT_MIN_WIDTH, requestedRight), sidebars - left)
    : 0
  return { left, center: width - left - right, right }
}
