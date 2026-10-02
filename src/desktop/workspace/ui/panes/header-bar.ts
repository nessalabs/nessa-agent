import type { PaneKey } from "../../../split-panes/model/pane-layout"

/**
 * What a header bar in the titlebar's row puts on itself and its spacer —
 * a session's pane, a widget's, or the window's — the one rule for all
 * three. Alone in the row (one pane, or the window over the panes) the bar
 * moves the window; beside other panes none of it may, or pressing it to
 * carry its pane would carry the window, and it carries its pane instead
 * (`split-panes/adapters/dom/drag.ts`). A pane's bar is held to its top
 * left as a drag's preview reshapes it.
 */
export function headerBar({
  pane,
  multi,
}: {
  /** The pane the bar heads; none for the window. */
  pane: PaneKey | null
  multi: boolean
}) {
  const movesWindow = !multi || pane === null
  return {
    bar: {
      "data-split-keeps": pane === null ? undefined : "top-left",
      "data-tauri-drag-region": movesWindow || undefined,
      "data-drag-pane": movesWindow || pane === null ? undefined : pane,
    },
    spacer: { "data-tauri-drag-region": movesWindow || undefined },
  } as const
}
