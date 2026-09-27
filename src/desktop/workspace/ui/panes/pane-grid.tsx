import { memo, useRef, type CSSProperties } from "react"
import { equalizePanes, resizePanes } from "../../adapters/store/commands"
import { useWorkspaceDispatch, useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectFailure, selectPlacements } from "../../adapters/store/selectors"
import type { EdgePlacement, PanePlacement } from "../../model/pane-sizing"
import { ResizeEdge } from "../chrome/resize-edge"
import { EmptyWorkspace } from "./empty-state"
import { Pane } from "./pane"
import "./panes.css"

/**
 * The chat area: every pane placed from the layout's fractions rather than
 * nested flex boxes, so a pane keeps its identity — and its DOM — when it
 * moves; and the edges between them, which resize the two panes either side.
 * It renders again only when the layout changes, never for a pane's content.
 */
export const PaneGrid = memo(function PaneGrid() {
  const { panes, edges } = useWorkspaceSelector(selectPlacements)
  const failure = useWorkspaceSelector(selectFailure)
  const multi = panes.length > 1
  return (
    <main className="workspace-chat" aria-label="Conversations">
      <div className="workspace-panes" data-multi={multi || undefined}>
        {panes.length === 0 ? <EmptyWorkspace failure={failure} /> : null}
        {/* In the order the panes were opened, not where they sit: a pane is placed
            by its fractions, so its node never moves in the document, and a
            moved pane keeps its scroll position and focus. */}
        {byAge(panes).map((placement) => (
          <Pane key={placement.key} placement={placement} multi={multi} />
        ))}
        {edges.map((edge) => (
          <PaneEdge key={edge.id} placement={edge} />
        ))}
      </div>
    </main>
  )
})

const byAge = (panes: readonly PanePlacement[]) =>
  [...panes].sort((a, b) => a.key - b.key)

/** Measures a pane's size along an axis, as drawn. */
function paneSize(key: number, axis: "x" | "y"): number {
  const box = document
    .querySelector<HTMLElement>(`[data-pane-key="${key}"]`)
    ?.getBoundingClientRect()
  return box ? (axis === "x" ? box.width : box.height) : 0
}

/**
 * The gutter between two columns, or two stacked panes, lit near the pointer.
 * A drag measures the two sides when it starts and asks for the share the
 * pointer implies; the layout holds each side at its readable minimum.
 */
const PaneEdge = memo(function PaneEdge({ placement }: { placement: EdgePlacement }) {
  const dispatch = useWorkspaceDispatch()
  const start = useRef({ before: 0, after: 0 })
  const axis = placement.edge.axis
  return (
    <ResizeEdge
      label={axis === "x" ? "Resize Columns" : "Resize Panes"}
      axis={axis}
      className="workspace-pane-edge"
      style={
        {
          "--cx": placement.x,
          "--cw": placement.width,
          "--ci": placement.column,
          "--cn": placement.columns,
          "--ry": placement.y,
          "--ri": placement.row,
          "--rn": placement.rows,
        } as CSSProperties
      }
      onStart={() => {
        start.current = {
          before: paneSize(placement.before, axis),
          after: paneSize(placement.after, axis),
        }
      }}
      onReset={() => dispatch(equalizePanes())}
      onMove={(delta) => {
        const pair = start.current.before + start.current.after
        if (pair <= 0) return
        dispatch(
          resizePanes({
            edge: placement.edge,
            fraction: (start.current.before + delta) / pair,
            pair,
          }),
        )
      }}
    />
  )
})
