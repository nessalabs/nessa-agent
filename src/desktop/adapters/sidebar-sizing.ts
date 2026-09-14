/** Fits requested pixel widths while reserving the workspace before sidebars. */
export function fitSidebarWidths(
  width: number,
  requestedLeft = 200,
  requestedRight = width - 550,
  priority: "left" | "right" = "left",
) {
  let left = requestedLeft > 0 ? Math.min(450, Math.max(200, requestedLeft)) : 0
  if (width - left < 350) left = width >= 550 ? 200 : 0
  if (priority === "right" && requestedRight > 0 && width - left - 350 < 160) {
    // Explicitly opening the right panel takes precedence over the other sidebar.
    // Keep both when the left can shrink to its minimum, otherwise release it.
    left = width >= 710 ? Math.min(left, width - 510) : 0
  }
  const rightSpace = Math.max(0, width - left - 350)
  const right =
    requestedRight > 0 && rightSpace >= 160
      ? Math.min(Math.max(160, requestedRight), rightSpace)
      : 0
  return { left, center: width - left - right, right }
}
