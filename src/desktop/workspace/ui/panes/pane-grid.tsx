import { memo, useEffect, useMemo, useRef, type CSSProperties } from "react"
import {
  edgeSides,
  equalizePanes,
  fitPanes,
  resizePanes,
} from "../../adapters/store/commands"
import {
  useWorkspaceDispatch,
  useWorkspaceSelector,
  useWorkspaceStore,
} from "../../adapters/store/hooks"
import {
  selectFailure,
  selectPanes,
  selectPlacements,
} from "../../adapters/store/selectors"
import { paneTabOrder } from "../../../split-panes/adapters/dom/tab-order"
import type { EdgePlacement, PanePlacement } from "../../../split-panes/model/pane-sizing"
import { ResizeEdge } from "../../../split-panes/ui/resize-edge"
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
  const dispatch = useWorkspaceDispatch()
  const { panes, edges } = useWorkspaceSelector(selectPlacements)
  const failure = useWorkspaceSelector(selectFailure)
  const multi = panes.length > 1
  const gridRef = useRef<HTMLDivElement>(null)
  const store = useWorkspaceStore()
  // Tab follows the panes as they are seen, not as they were opened.
  const tabOrder = useMemo(
    () => paneTabOrder(() => selectPanes(store.getState())),
    [store],
  )
  // Whatever changes the panes' room — the window, a side column — every pane
  // is held to the readable size again. Observed as laid out, after layout and
  // before paint, so no frame shows a pane below it.
  useEffect(() => {
    const grid = gridRef.current
    if (!grid) return
    const observer = new ResizeObserver(() => dispatch(fitPanes()))
    observer.observe(grid)
    return () => observer.disconnect()
  }, [dispatch])
  return (
    <main className="workspace-chat" aria-label="Conversations">
      <div
        className="workspace-panes"
        ref={gridRef}
        data-multi={multi || undefined}
        onKeyDown={tabOrder}
      >
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

/**
 * The gutter between two columns, or two stacked panes, lit near the pointer.
 * A drag takes the two sides' sizes when it starts — from the room as laid
 * out, never a pane mid-flight — and asks for the share the pointer implies;
 * the layout holds each side at its readable minimum.
 */
const PaneEdge = memo(function PaneEdge({ placement }: { placement: EdgePlacement }) {
  const dispatch = useWorkspaceDispatch()
  const start = useRef({ before: 0, after: 0 })
  const axis = placement.edge.axis
  return (
    <ResizeEdge
      // The part of the two sides' room the one before the edge takes, in percent.
      value={{ now: placement.leading * 100, min: 0, max: 100 }}
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
        start.current = dispatch(edgeSides(placement.edge)) ?? { before: 0, after: 0 }
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
