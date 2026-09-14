/** Fits requested pixel widths while reserving the workspace before sidebars. */
export function fitSidebarWidths(
  width: number,
  requestedLeft = 200,
  requestedRight = width - 550,
) {
  let left = requestedLeft > 0 ? Math.min(450, Math.max(200, requestedLeft)) : 0
  if (width - left < 350) left = width >= 550 ? 200 : 0
  const rightSpace = Math.max(0, width - left - 350)
  const right =
    requestedRight > 0 && rightSpace >= 160
      ? Math.min(Math.max(160, requestedRight), rightSpace)
      : 0
  return { left, center: width - left - right, right }
}
