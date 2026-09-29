/**
 * Split panes: a grid of panes placed by fractions, resized by the edges
 * between them, and rearranged by dragging a pane — or an item from outside
 * the grid — onto a zone of another, with a preview that draws every pane at
 * the shape the drop would give it. Any desktop surface can wrap it; the
 * workspace does. See ADR 253 for the contract and ADR 238 › _Drag and drop_
 * for the behaviour.
 *
 * ```text
 *   host ──implements──▶ SplitPanesSource (application/ports.ts)
 *     │                    layout · subscribe · watched · holds · targetable
 *     │                    measure · commitDrop · resize · equalize · fit
 *     │
 *     ├──▶ <SplitPanes source renderPane empty>        ui/split-panes.tsx
 *     │      ├──▶ renderPane({ placement, frame, multi }) ──▶ the host's pane root
 *     │      └──▶ ResizeEdge ──▶ usePointerResize       ui/, adapters/dom/
 *     ├──▶ useSplitPanesDrag(root, source, options)   adapters/dom/drag.ts
 *     │      └──▶ model/drag.ts (stepDrag) ──▶ model/drop.ts (aimAt, dropOutcome)
 *     └──▶ FlipScope                                   adapters/dom/flip.tsx
 *
 *   model/pane-layout.ts ◀── model/pane-sizing.ts ◀── model/drop.ts ◀── model/drag.ts
 * ```
 *
 * An arrow points from what uses to what it uses. The model is pure — a
 * host's own model and use cases import it by its files, which is the one
 * way past this map; everything else a host takes from here. The module
 * never stores a layout: a drop, a resize, an equalize or a fit is the
 * host's to apply, through the source, so its rules keep one owner. The grid
 * adds no element around a pane: the host spreads the grid's `frame` on its
 * own root, which FLIP and the drag's preview move. `ui/split-panes.css`
 * places, resizes and carries panes; what a pane looks like is the host's.
 *
 * A host marks its page for the drag: `data-drag-pane` (a pane's key, on
 * what carries it) and `data-drag-item` (an item's id, outside the grid);
 * `data-split-keeps` (`top-left`, `foot`, `middle`) on the parts of a pane a
 * preview holds to a point, `data-split-through` on a wrapper to look
 * inside, `data-split-scroll` on the scroller a copy shows one screen of.
 */
export { useSplitPanesDrag, type SplitPanesDragOptions } from "./adapters/dom/drag"
export { classes, gridOf, marks } from "./adapters/dom/marks"
export { FlipScope } from "./adapters/dom/flip"
export { SplitPanes, type PaneFrame, type ShownPane } from "./ui/split-panes"
export type { Drop, EdgeMove, SplitPanesSource } from "./application/ports"
