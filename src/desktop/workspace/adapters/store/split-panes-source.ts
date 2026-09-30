/**
 * The workspace as the split panes' source (`SplitPanesSource`): the layout
 * the store holds, read as the store changes — synchronously, so a drag ends
 * the moment anything under it moves — and every change carried out by the
 * workspace's own commands, which hold it to the room the page measures and
 * fold the sidebar where that is what makes it fit.
 */
import type { SplitPanesSource } from "../../../split-panes"
import type { DesktopStore } from "../../../store"
import { paneItemOf } from "../../model/pane-item"
import { commitDrop, equalizePanes, fitPanes, measureRoom, resizePanes } from "./commands"

export function workspaceSplitPanes(store: DesktopStore): SplitPanesSource {
  const workspace = () => store.getState().workspace
  return {
    layout: () => workspace().panes,
    subscribe: (onChange) => store.subscribe(onChange),
    // What the content region shows, and the side columns: a drag read both.
    watched: () => [workspace().content, workspace().chrome],
    // Carried from outside the grid: a listed session's row, by its pane item.
    holds: (key) => {
      const item = paneItemOf(key)
      return (
        item?.kind === "session" && Object.hasOwn(workspace().sessions, item.sessionId)
      )
    },
    // Only panes a person can see are aimed at: none under the Agents overview.
    targetable: () => workspace().content === "panes",
    measure: () => store.dispatch(measureRoom()),
    commitDrop: (drop) => store.dispatch(commitDrop(drop)),
    resize: (move) => store.dispatch(resizePanes(move)),
    equalize: () => store.dispatch(equalizePanes()),
    fit: () => store.dispatch(fitPanes()),
  }
}
