import {
  Fragment,
  memo,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react"
import type { SplitPanesSource } from "../application/ports"
import { paneTabOrder } from "../adapters/dom/tab-order"
import type { Column, PaneKey } from "../model/pane-layout"
import {
  edgeSides,
  placements,
  type EdgePlacement,
  type PanePlacement,
} from "../model/pane-sizing"
import { ResizeEdge } from "../../ui/resize-edge"
import { classes, marks } from "../adapters/dom/marks"
import "./split-panes.css"

/** Which pane is where, ignoring the shares a resize changes. */
function arrangementOf(columns: readonly Column[] | null): string {
  if (!columns) return ""
  return columns
    .map((column) => `${column.key}:${column.panes.map((pane) => pane.key).join(",")}`)
    .join("|")
}

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
  readonly [marks.corner]: true | undefined
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
  const gridRef = useRef<HTMLDivElement>(null)
  // What is on screen lags a new arrangement. The frame that commits a split
  // or a drop runs its script only — the grid does not render for it. The
  // next frame is when a drop's copy lets go and, in the workspace, when a
  // side column folds for the new count. The frame after that is the column
  // title's glide. The third frame paints the new boxes, so none of those
  // share a paint. A resize of the same panes paints on the frame it happens,
  // so a separator follows the pointer. A draw already handed to React is
  // still open: a later callback in that frame can put the source back
  // before the commit, and the grid replaces the draw. The first paint
  // uses the layout the grid mounted with.
  const [drawn, setDrawn] = useState(() => source.layout()?.columns ?? null)
  const drawnRef = useRef(drawn)
  drawnRef.current = drawn
  const { panes, edges } = useMemo(
    () => (drawn ? placements(drawn) : noPlacements),
    [drawn],
  )
  useEffect(() => {
    type Columns = NonNullable<ReturnType<SplitPanesSource["layout"]>>["columns"] | null
    let frames: number[] = []
    let armed = false
    let waiting: Columns = null
    // Columns passed to React that this commit has not rendered. The ref
    // above updates in render, so a callback later in the same frame still
    // sees the previous arrangement.
    let pending: { columns: Columns } | null = null
    const cancel = () => {
      for (const id of frames) cancelAnimationFrame(id)
      frames = []
    }
    const hold = (on: boolean) => {
      gridRef.current
        ?.closest<HTMLElement>("[data-workspace]")
        ?.toggleAttribute("data-spare-held", on)
    }
    const draw = (columns: Columns) => {
      pending = { columns }
      setDrawn(columns)
    }
    const arm = (next: typeof waiting) => {
      if (armed && waiting === next) return
      cancel()
      armed = true
      waiting = next
      // The queued draw is another arrangement. Replace it before React
      // commits it, then count this arrangement's own frames.
      if (pending && pending.columns !== next) draw(drawnRef.current)
      const queue = (left: number) => {
        const id = requestAnimationFrame(() => {
          // The click paints nothing new. The hold starts on the next frame,
          // before the window records the fold.
          if (left === 3) hold(next !== drawnRef.current)
          if (left > 1) {
            queue(left - 1)
            return
          }
          frames = []
          armed = false
          waiting = null
          draw(next)
        })
        frames.push(id)
      }
      queue(3)
    }
    // Subscribing does not call back. A layout that landed before this
    // effect, or a source this grid just took, is read now and still waits
    // its frames.
    const reconcile = () => {
      const next = source.layout()?.columns ?? null
      const shown = drawnRef.current
      if (pending?.columns === shown) pending = null
      // This arrangement is already the update React will commit.
      if (pending?.columns === next) return
      if (next === shown) {
        // The source came back. A draw of the arrangement it left is still
        // queued, and returning here would let that commit land.
        const stale = pending !== null
        if (stale) draw(shown)
        if (!armed && !stale) return
        cancel()
        armed = false
        waiting = null
        hold(false)
        return
      }
      // The same panes, resized. Waiting three frames would restart on every
      // pointer frame, and the separator would not follow the pointer.
      if (arrangementOf(next) === arrangementOf(shown)) {
        cancel()
        armed = false
        waiting = null
        hold(false)
        draw(next)
        return
      }
      arm(next)
    }
    const unsubscribe = source.subscribe(reconcile)
    reconcile()
    return () => {
      cancel()
      unsubscribe()
    }
  }, [source])
  // The frame that first paints a new arrangement skips scene text. The text
  // is already laid out at a fixed width; painting it again is the resize.
  // A resize of the same panes does not, so a drag does not hide the scene.
  const shape = arrangementOf(drawn)
  useLayoutEffect(() => {
    const grid = gridRef.current
    if (!grid) return
    grid.setAttribute(marks.painting, "")
    const id = requestAnimationFrame(() => grid.removeAttribute(marks.painting))
    return () => cancelAnimationFrame(id)
  }, [shape])
  // The workspace holds the sidebar in the row until these boxes paint.
  // Folding it out of the row on the earlier frame widens the one pane and
  // lays its text out, then the boxes lay it out again.
  useLayoutEffect(() => {
    const workspace = gridRef.current?.closest<HTMLElement>("[data-workspace]")
    if (!workspace) return
    const live = source.layout()?.columns ?? null
    workspace.toggleAttribute("data-spare-held", drawn !== live)
    return () => workspace.removeAttribute("data-spare-held")
  }, [drawn, source])
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
 * The gutter between two columns, or two stacked panes, lit on hover or focus.
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
      className={classes.paneEdge}
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
