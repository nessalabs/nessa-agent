/**
 * Every name on the page a host writes for split panes, or reads from them —
 * published here, once. A host's code and tests derive them where they can,
 * and spell them where JSX needs a literal attribute; either way they are
 * held to this set. Each begins `data-split-`, `data-drag-` or `split-panes-`, so
 * a host's stylesheets, which cannot import this, are held to it by what
 * they spell: `src/desktop/styles.test.ts` refuses any name of those three
 * families, written out whole in a host's stylesheet, code or test, or in a
 * verification script — or read as `dataset.splitX` / `dataset.dragX` — that
 * is not one of these. A name built at runtime (a template, a joined string)
 * is not something it can read.
 *
 * The frame's own names (`data-pane-key`, `data-flip`, `data-flip-id`) are
 * spread by the host from the grid's `frame` rather than spelled, and FLIP's
 * `data-flip="slide"` is FLIP's markup; they keep the names FLIP and the
 * verification scripts have always used.
 */
export const marks = {
  // ——— What a host writes ———
  /** A pane's key, on what carries the pane (its header). */
  dragPane: "data-drag-pane",
  /** An item's id, on what carries it from outside the grid (a row). */
  dragItem: "data-drag-item",
  /** What a pane's part keeps to as a preview reshapes it: `top-left`, `foot`, `middle`. */
  keeps: "data-split-keeps",
  /** A wrapper inside a pane whose children are its parts. */
  through: "data-split-through",
  /** The scroller a pane's copy shows one screen of; its first child is the content. */
  scroll: "data-split-scroll",

  // ——— What the module sets, which a host may style or read ———
  /** The grid itself (`gridOf`). */
  grid: "data-split-grid",
  /** On the grid while it holds more than one pane. */
  multi: "data-split-multi",
  /** On the top-left pane's root, which clears the window's controls when nothing is beside it. */
  corner: "data-split-corner",
  /** On a pane a drag's preview puts in the corner (`yes`) or takes out of it (`no`). */
  dragCorner: "data-drag-corner",
  /** On the FLIP root while anything flies. */
  flipping: "data-split-flipping",
  /** On the drag's root while a preview draws panes away from where they are laid out. */
  reflow: "data-drag-reflow",
  /** On the drag's root while carrying (`pane` or `item`), and on what was pressed. */
  carrying: "data-drag-carrying",
  /** On the pane being carried: its slot, left behind. */
  lifted: "data-drag-lifted",
  /** On the copy while it waits, unseen, for the press to become a drag. */
  waiting: "data-drag-waiting",
  /** On the drag's root while a drop shown would take the host's spare room (`takesSpare`). */
  takesSpare: "data-drag-takes-spare",
} as const

/** The module's own elements a host may style or find. */
export const classes = {
  grid: "split-panes-grid",
  /** The carried copy; the host styles its own content inside it. */
  ghost: "split-panes-ghost",
  /** The layer everything carried is drawn in. */
  layer: "split-panes-layer",
  /** What moves with the pointer, one to one. */
  carrier: "split-panes-carrier",
  /** Where a drop would land. */
  placeholder: "split-panes-placeholder",
  /** Over the page while carrying. */
  shield: "split-panes-shield",
} as const

/** The grid under `root`, if one is drawn: what a host measures the panes' room by. */
export function gridOf(root: ParentNode): HTMLElement | null {
  return root.querySelector<HTMLElement>(`[${marks.grid}]`)
}
