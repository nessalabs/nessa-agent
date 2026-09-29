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
 *     │      └──▶ the desktop's ResizeEdge              src/desktop/ui/resize-edge.tsx
 *     ├──▶ useSplitPanesDrag(root, source, options)   adapters/dom/drag.ts
 *     │      └──▶ model/drag.ts (stepDrag) ──▶ model/drop.ts (aimAt, dropOutcome)
 *     ├──▶ FlipScope                                   adapters/dom/flip.tsx
 *     └──▶ marks, classes, gridOf                      adapters/dom/marks.ts
 *
 *   model/pane-layout.ts ◀── model/pane-sizing.ts ◀── model/drop.ts ◀── model/drag.ts
 * ```
 *
 * An arrow points from what uses to what it uses. A host takes what it uses
 * from here; any module may import the pure model's files, and a host's own
 * model and use cases, which may not import React, must; a test may take
 * `testing.ts`. The module imports no host — only the desktop window's shared
 * parts. `scripts/architecture/split-panes-boundary.mjs` holds both.
 *
 * The module keeps no layout: a drop, a resize, an equalize or a fit is
 * asked of the source (`split-panes.test.tsx`, "resizes, evens and fits
 * through the source"; the drag's tests read every drop from it), so a
 * host's rules keep one owner. The grid adds no element around a pane: the
 * host spreads the grid's `frame` on its own root, which FLIP and the drag's
 * preview move. `ui/split-panes.css` places and carries panes; what a pane
 * looks like is the host's.
 *
 * What a host may see of the page is published (`marks`, `classes`) and
 * nothing else is its to spell; `src/desktop/styles.test.ts` refuses an
 * unpublished name of the three families written out whole in a host file
 * or a verification script, or read through `dataset` (not one built at
 * runtime):
 *
 * - it writes `data-drag-pane` (a pane's key, on what carries it),
 *   `data-drag-item` (an item's id, outside the grid), and on a pane's
 *   parts `data-split-keeps` (`top-left`, `foot`, `middle`),
 *   `data-split-through` (a wrapper to look inside) and `data-split-scroll`
 *   (what a copy shows one screen of);
 * - it may style or read the grid (`.split-panes-grid`, `data-split-grid`,
 *   found with `gridOf`, and `data-split-multi`), a pane's corner
 *   (`data-split-corner`, and `data-drag-corner` while a preview moves it),
 *   motion in progress (`data-split-flipping`, `data-drag-reflow`), a drag
 *   (`data-drag-carrying`, `data-drag-lifted`, `data-drag-waiting`,
 *   `data-drag-takes-spare`), and the carried copy and its layer
 *   (`.split-panes-ghost`, `-layer`, `-carrier`, `-placeholder`, `-shield`);
 * - the frame spreads `data-pane-key`, `data-flip` and `data-flip-id`, and a
 *   host marks a column that slides with FLIP's `data-flip="slide"`.
 */
export { useSplitPanesDrag, type SplitPanesDragOptions } from "./adapters/dom/drag"
export { classes, gridOf, marks } from "./adapters/dom/marks"
export { FlipScope } from "./adapters/dom/flip"
export { SplitPanes, type PaneFrame, type ShownPane } from "./ui/split-panes"
export type { Drop, EdgeMove, SplitPanesSource } from "./application/ports"
