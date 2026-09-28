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
 *     ├──▶ useSplitPanesDrag(root, source, options)   adapters/dom/drag.ts
 *     │      └──▶ model/drag.ts (stepDrag) ──▶ model/drop.ts (aimAt, dropOutcome)
 *     ├──▶ FlipScope                                   adapters/dom/flip.tsx
 *     └──▶ ResizeEdge ──▶ usePointerResize             ui/, adapters/dom/
 *
 *   model/pane-layout.ts ◀── model/pane-sizing.ts ◀── model/drop.ts ◀── model/drag.ts
 * ```
 *
 * An arrow points from what uses to what it uses. The model is pure — a
 * host's own model and use cases import it by its files, which is the one
 * way past this map; everything else a host takes from here. The module
 * never stores a layout: a drop, a resize, an equalize or a fit is the
 * host's to apply, through the source, so its rules keep one owner.
 *
 * A host marks its page for the drag: `data-drag-pane` (a pane's key, on
 * what carries it) and `data-drag-item` (an item's id, outside the grid);
 * `data-split-keeps` (`top-left`, `foot`, `middle`) on the parts of a pane a
 * preview holds to a point, `data-split-through` on a wrapper to look
 * inside, `data-split-scroll` on the scroller a copy shows one screen of.
 */
export { useSplitPanesDrag, type SplitPanesDragOptions } from "./adapters/dom/drag"
export { FlipScope } from "./adapters/dom/flip"
export { paneTabOrder } from "./adapters/dom/tab-order"
export { ResizeEdge } from "./ui/resize-edge"
export type { Drop, EdgeMove, SplitPanesSource } from "./application/ports"
