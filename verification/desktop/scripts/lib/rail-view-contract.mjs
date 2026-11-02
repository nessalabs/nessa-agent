/**
 * What `rail-corner` and `rail-view-inert` decide from boxes the page measured.
 *
 * Devices leaves the bottom-left corner clear (`studio: false` on its rail
 * item): its cards run under that corner, so "nessa Studio" there is the
 * regression. A full view's titlebar sits above the view and its free room is
 * the window's drag region. A hit there is the window, not a workspace card.
 */

/** The words start before the toggle's right edge, so they run under it. */
export function studioUnderToggleFailure(label, studio, toggle) {
  if (!studio || !toggle) return []
  if (studio[0] >= toggle[0] + toggle[2]) return []
  return [`${label}: "nessa Studio" at x ${studio[0]} is under the toggle`]
}

/**
 * Agents, with the sidebar shown, draws "nessa Studio". Rail open and rail
 * closed are judged apart: opening the rail steps the words clear of the
 * pill, and that step is not the toggle moving.
 */
export function agentsStudioMissingFailure(label, studio) {
  if (studio) return []
  return [`${label}: "nessa Studio" is missing in Agents`]
}

/**
 * Devices draws no "nessa Studio". The toggle stays on the place Agents used.
 */
export function devicesCornerFailures(label, inView, viewToggle, toggle) {
  const failures = []
  if (inView)
    failures.push(
      `${label}: Devices draws "nessa Studio" at [${inView}]; that corner stays clear`,
    )
  if (!toggle || viewToggle?.join() !== toggle.join())
    failures.push(`${label}, Devices: the toggle is at [${viewToggle}], not [${toggle}]`)
  return failures
}

const exposed = "plugin view exposes the workspace cards or controls above it"

/**
 * The full view covers the workspace from the rail's right edge to the
 * window's other edges, top included. `titlebarHit` is where a press in the
 * titlebar's free room lands. Under a full view that is the drag region, on
 * purpose, so it is not part of this verdict.
 */
export function railViewCoverFailures(chrome) {
  if (!chrome?.view) return [exposed]
  const covered =
    chrome.top <= 1 &&
    chrome.bottom >= chrome.windowHeight - 1 &&
    chrome.right >= chrome.windowWidth - 1 &&
    Math.abs(chrome.left - chrome.railRight) <= 1 &&
    chrome.coversContent === true &&
    chrome.hitWorkspace !== true &&
    Array.isArray(chrome.navigation) &&
    chrome.navigation.join() === "side-rail"
  return covered ? [] : [exposed]
}
