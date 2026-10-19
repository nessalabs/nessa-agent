/**
 * What the window shows, as its agents are told it: the one reading of the
 * workspace's state into a `ShownWindow` (`model/shown-window.ts`), and the
 * layout saying it is on screen.
 *
 * ```text
 * state ─▶ shownWindow ─▶ arrangement (null: nothing is said)
 *                     ├─▶ sidebar, session list: drawn now (`drawnColumns`)
 *                     ├─▶ showing: panes | agents | widget (`content`)
 *                     └─▶ columns ─▶ panes: share %, focused, item
 *                                         item: listed session ─▶ conversation
 *                                               draft ─▶ draft
 *                                               widget ─▶ widget
 * ```
 */
import { paneItemOf } from "../../model/pane-item"
import {
  percents,
  type ShownItem,
  type ShownWindow,
  type WindowArrangement,
} from "../../model/shown-window"
import {
  contentKind,
  draftOf,
  drawnColumns,
  sessionOf,
  windowWidget,
  type WorkspaceState,
} from "../workspace-state"

/** The layout drawing the panes is on screen now, or none is (`null`). */
export function arrangementShown(
  state: WorkspaceState,
  { arrangement }: { arrangement: WindowArrangement | null },
): WorkspaceState {
  return state.arrangement === arrangement ? state : { ...state, arrangement }
}

/**
 * What the window shows now, or `null` when it draws no panes: no layout of
 * panes is on screen, or the index has not placed any yet.
 */
export function shownWindow(state: WorkspaceState): ShownWindow | null {
  const { arrangement, panes } = state
  if (arrangement === null || panes === null) return null
  const drawn = drawnColumns(state.chrome)
  const widget = windowWidget(state.content)
  const kind = contentKind(state.content)
  const widths = percents(panes.columns.map((column) => column.share))
  return {
    arrangement,
    sidebar: drawn.sidebar,
    sessionList: arrangement === "columns" && drawn.sessionList,
    showing:
      kind === "widget" && widget
        ? { kind, widget }
        : { kind: kind === "agents" ? kind : "panes" },
    columns: panes.columns.map((column, index) => {
      const heights = percents(column.panes.map((pane) => pane.share))
      return {
        widthPercent: widths[index],
        panes: column.panes.map((pane, row) => ({
          heightPercent: heights[row],
          focused: pane.key === panes.focused,
          item: shownItem(state, pane.item),
        })),
      }
    }),
  }
}

/** A pane's item as shown: a session not started yet is a draft, any other a conversation. */
function shownItem(state: WorkspaceState, key: string): ShownItem {
  const item = paneItemOf(key)
  // Never: every pane's key is `paneItemKey`'s. Said as a pane with nothing
  // started in it, rather than leaving a column without the pane.
  if (item === null) return { kind: "draft" }
  if (item.kind === "widget") return { kind: "widget", widget: item.widget }
  // A draft stays one until the source lists it.
  if (draftOf(state, item.sessionId) && !sessionOf(state, item.sessionId))
    return { kind: "draft" }
  return { kind: "conversation", sessionId: item.sessionId }
}
