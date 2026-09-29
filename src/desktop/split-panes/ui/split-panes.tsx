import {
  Fragment,
  memo,
  useEffect,
  useMemo,
  useRef,
  useSyncExternalStore,
  type CSSProperties,
  type ReactNode,
} from "react"
import type { SplitPanesSource } from "../application/ports"
import { paneTabOrder } from "../adapters/dom/tab-order"
import type { PaneKey } from "../model/pane-layout"
import {
  edgeSides,
  placements,
  type EdgePlacement,
  type PanePlacement,
} from "../model/pane-sizing"
import { ResizeEdge } from "../../ui/resize-edge"
import { classes, marks } from "../adapters/dom/marks"
import "./split-panes.css"

/**
 * What a pane's root takes from the grid, spread onto the host's own element
 * — no element is added around it, because FLIP and a drag's preview move
 * that root and counter-scale its direct children. `style` places it (the
 * custom properties `split-panes.css` places a pane by); the rest name it to
 * FLIP (`data-flip`, `data-flip-id`) and to the drag (`data-pane-key`), and
 * mark the top-left pane, which clears the window's controls when nothing is
 * beside it (`data-split-corner`).
 */
export interface PaneFrame {
  readonly style: CSSProperties
  readonly "data-flip": "pane"
  readonly "data-flip-id": PaneKey
  readonly "data-pane-key": PaneKey
  readonly "data-split-corner": true | undefined
}

/** A pane as the grid hands it to the host to render. */
export interface ShownPane {
  readonly placement: PanePlacement
  readonly frame: PaneFrame
  /** Whether the grid holds more than one pane. */
  readonly multi: boolean
}

/** The frame of a pane at `placement`. */
export function paneFrame(placement: PanePlacement): PaneFrame {
  return {
    style: {
      "--cx": placement.x,
      "--cw": placement.width,
      "--ci": placement.column,
      "--cn": placement.columns,
      "--ry": placement.y,
      "--rh": placement.height,
      "--ri": placement.row,
      "--rn": placement.rows,
    } as CSSProperties,
    "data-flip": "pane",
    "data-flip-id": placement.key,
    "data-pane-key": placement.key,
    [marks.corner]: placement.corner || undefined,
  }
}

const noPlacements: ReturnType<typeof placements> = { panes: [], edges: [] }

/** In the order the panes were opened, not where they sit. */
const byAge = (panes: readonly PanePlacement[]) =>
  [...panes].sort((a, b) => a.key - b.key)

/**
 * The grid: every pane placed from the layout's fractions rather than nested
 * flex boxes, so a pane keeps its identity — and its DOM — when it moves; and
 * the edges between them, which resize the two panes either side. It reads
 * the layout the source holds, and renders again only when the panes'
 * arrangement or sizes change — never for focus, never for a pane's content.
 * `renderPane` renders each pane's root, spreading its `frame` on it; `empty`
 * is shown while there are no panes. Whatever changes the grid's room — the
 * window, what the host puts beside it — the source is asked to fit the
 * panes to it again (`fit`), observed as laid out, after layout and before
 * paint, so no frame shows a pane below its readable size.
 */
export const SplitPanes = memo(function SplitPanes({
  source,
  renderPane,
  empty,
}: {
  source: SplitPanesSource
  renderPane: (pane: ShownPane) => ReactNode
  empty?: ReactNode
}) {
  const columns = useSyncExternalStore(
    source.subscribe,
    () => source.layout()?.columns ?? null,
  )
  const { panes, edges } = useMemo(
    () => (columns ? placements(columns) : noPlacements),
    [columns],
  )
  const multi = panes.length > 1
  // In the order the panes were opened, not where they sit: a pane is placed
  // by its fractions, so its node never moves in the document, and a moved
  // pane keeps its scroll position and focus.
  const shown = useMemo(
    () =>
      byAge(panes).map((placement): ShownPane => ({
        placement,
        frame: paneFrame(placement),
        multi,
      })),
    [panes, multi],
  )
  const gridRef = useRef<HTMLDivElement>(null)
  // Tab follows the panes as they are seen, not as they were opened.
  const tabOrder = useMemo(() => paneTabOrder(() => source.layout()), [source])
  useEffect(() => {
    const grid = gridRef.current
    if (!grid) return
    const observer = new ResizeObserver(() => source.fit())
    observer.observe(grid)
    return () => observer.disconnect()
  }, [source])
  return (
    <div
      className={classes.grid}
      {...{ [marks.grid]: "" }}
      ref={gridRef}
      {...{ [marks.multi]: multi || undefined }}
      onKeyDown={tabOrder}
    >
      {panes.length === 0 ? empty : null}
      {shown.map((pane) => (
        <Fragment key={pane.placement.key}>{renderPane(pane)}</Fragment>
      ))}
      {edges.map((edge) => (
        <PaneEdge key={edge.id} placement={edge} source={source} />
      ))}
    </div>
  )
})

/**
 * The gutter between two columns, or two stacked panes, lit near the pointer.
 * A drag takes the two sides' sizes when it starts — from the room as laid
 * out, never a pane mid-flight — and asks for the share the pointer implies;
 * the layout holds each side at its readable minimum.
 */
const PaneEdge = memo(function PaneEdge({
  placement,
  source,
}: {
  placement: EdgePlacement
  source: SplitPanesSource
}) {
  const start = useRef({ before: 0, after: 0 })
  const axis = placement.edge.axis
  return (
    <ResizeEdge
      // The part of the two sides' room the one before the edge takes, in percent.
      value={{ now: placement.leading * 100, min: 0, max: 100 }}
      label={axis === "x" ? "Resize Columns" : "Resize Panes"}
      axis={axis}
      className="split-panes-pane-edge"
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
        const layout = source.layout()
        const room = source.measure()
        start.current = (layout && room
          ? edgeSides(layout, placement.edge, room)
          : null) ?? {
          before: 0,
          after: 0,
        }
      }}
      onReset={() => source.equalize()}
      onMove={(delta) => {
        const pair = start.current.before + start.current.after
        if (pair <= 0) return
        source.resize({
          edge: placement.edge,
          fraction: (start.current.before + delta) / pair,
          pair,
        })
      }}
    />
  )
})
