/** The left sidebar's widths: it opens at the default, and closes below the minimum. */
export const LEFT_MIN_WIDTH = 200
export const LEFT_DEFAULT_WIDTH = 250
export const LEFT_MAX_WIDTH = 450

/** Fits requested pixel widths while reserving the workspace before sidebars. */
export function fitSidebarWidths(
  width: number,
  requestedLeft = LEFT_DEFAULT_WIDTH,
  requestedRight = 400,
  workspaceCollapsed = false,
) {
  let left =
    requestedLeft > 0
      ? Math.min(LEFT_MAX_WIDTH, Math.max(LEFT_MIN_WIDTH, requestedLeft))
      : 0
  if (workspaceCollapsed) return { left, center: 0, right: width - left }
  if (width - left < 350) left = width >= LEFT_MIN_WIDTH + 350 ? LEFT_MIN_WIDTH : 0
  const rightSpace = Math.max(0, width - left - 350)
  const right =
    requestedRight > 0 && rightSpace > 0
      ? Math.min(Math.max(160, requestedRight), rightSpace)
      : 0
  return { left, center: width - left - right, right }
}
