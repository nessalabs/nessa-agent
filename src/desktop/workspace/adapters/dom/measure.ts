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
 *
 * The room last read is what a click places into, so the click does not lay
 * the page out. The grid and the sidebar are watched after that read: a
 * resize or a sidebar toggle writes the room again from the layout that
 * just happened, and a later click sees the geometry on screen.
 */
import { gridOf } from "../../../split-panes"
import { paneLimits } from "../../../split-panes/model/pane-layout"
import type { PaneRoom } from "../../../split-panes/model/pane-sizing"

/** The room last read from the page, so a click can place a pane without laying out. */
let painted: PaneRoom | undefined
let watched: HTMLElement | undefined
let watcher: ResizeObserver | undefined

/** The room as last laid out, without reading the page. Nothing until something has measured. */
export function paintedRoom(): PaneRoom | undefined {
  return painted
}

function roomOf(grid: HTMLElement): PaneRoom {
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

/** Follow the grid and the sidebar, so a later layout replaces the cached room. */
function watch(grid: HTMLElement) {
  if (watched === grid || typeof ResizeObserver === "undefined") return
  watcher?.disconnect()
  watched = grid
  watcher = new ResizeObserver(() => {
    painted = roomOf(grid)
  })
  watcher.observe(grid)
  const sidebar = grid.closest("[data-workspace]")?.querySelector(".workspace-sidebar")
  if (sidebar) watcher.observe(sidebar)
}

export function measureWorkspace(page: ParentNode = document): PaneRoom | undefined {
  const grid = gridOf(page)
  if (!grid) return undefined
  const room = roomOf(grid)
  painted = room
  watch(grid)
  return room
}
