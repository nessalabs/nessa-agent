/** The workspace carries a small title card and highlights a target without moving live chats. */
import type { SplitPanesDragOptions } from "../../../split-panes"
import { focusedPaneAttribute } from "./focus"

/** The card stays this size over every target. */
export const dragCardSize = { width: 280, height: 44 } as const

/** A pane or list row's visible identity, without any conversation or input copy. */
export const dragCard: SplitPanesDragOptions["copyOf"] = (
  _carried,
  { pressed, pane, picture },
) => {
  const origin = pane ?? pressed
  const card = document.createElement("div")
  card.className = "workspace-drag-card"
  const icon =
    origin.querySelector(".workspace-agent-tile") ??
    origin.querySelector(".workspace-pane-name > svg")
  if (icon) card.append(picture(icon))
  const name = origin.querySelector(".workspace-pane-title, .workspace-session-title")
  const title = document.createElement("span")
  title.className = "workspace-drag-title"
  title.dir = "auto"
  title.textContent =
    name?.textContent ?? pane?.getAttribute("aria-label") ?? pressed.textContent ?? ""
  card.append(title)
  return card
}

/**
 * The side columns drawn under the workspace's root as the press begins —
 * the sidebar docked or revealed from the edge over the panes, the session
 * list docked — which are never targets, whatever is under them.
 */
export function sideColumns(root: HTMLElement): readonly Element[] {
  const { sidebar, list } = root.dataset
  return [
    sidebar === "open" || "peek" in root.dataset
      ? root.querySelector(".workspace-sidebar")
      : null,
    list === "open" ? root.querySelector(".workspace-list") : null,
  ].flatMap((column) => (column ? [column] : []))
}

/** Everything the workspace adds to a drag, built once for its root. */
export function workspaceDragOptions(): SplitPanesDragOptions {
  return {
    copyOf: dragCard,
    copySize: dragCardSize,
    previewPanes: false,
    covered: sideColumns,
    stripped: ["data-tauri-drag-region", focusedPaneAttribute],
  }
}
