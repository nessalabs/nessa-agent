/**
 * The room the panes have on the page: the pane grid's box as laid out, and
 * what the sidebar would give up if it folded. The one place the window is
 * measured for a change of layout, handed to the commands from composition
 * (`WorkspaceDependencies.measure`), so a split asked for by a key, a menu,
 * a drop or an agent is held to the same room.
 *
 * Layout sizes only (`offsetWidth`, `offsetHeight`): a pane flying by FLIP
 * is drawn with a transform, and a change asked for mid-flight must be
 * measured against where things are, not where the motion shows them. The
 * panes stay laid out under the Agents overview, so what is measured while it
 * is open is the room they have.
 */
import { paneLimits } from "../../../split-panes/model/pane-layout"
import type { PaneRoom } from "../../../split-panes/model/pane-sizing"

export function measureWorkspace(page: ParentNode = document): PaneRoom | undefined {
  const grid = page.querySelector<HTMLElement>(".workspace-panes")
  if (!grid) return undefined
  const root = grid.closest<HTMLElement>("[data-workspace]")
  const sidebar =
    root?.dataset.sidebar === "open"
      ? root.querySelector<HTMLElement>(".workspace-sidebar")
      : null
  return {
    width: grid.offsetWidth,
    height: grid.offsetHeight,
    spare: sidebar ? sidebar.offsetWidth + paneLimits.gutter : 0,
  }
}
