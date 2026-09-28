/**
 * Tab walks the panes in the order they are seen — down each column, then
 * across — whatever order they were opened in. A pane keeps its place in the
 * page (so a move keeps its scroll and focus: `ui/panes/pane-grid.tsx`), so
 * the browser's own order is the order of opening; at a pane's first or last
 * stop, Tab is taken here to the next pane on screen instead, and past the
 * last to what follows the panes.
 */
import type { KeyboardEvent as ReactKeyboardEvent } from "react"
import { panesOf, type PaneLayout } from "../../../split-panes/model/pane-layout"

const tabbable =
  'a[href], button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])'

/** What Tab stops on in `scope`, in the page's order, leaving out what cannot take focus now. */
function stops(scope: ParentNode): HTMLElement[] {
  return [...scope.querySelectorAll<HTMLElement>(tabbable)].filter(
    (element) =>
      element.tabIndex >= 0 &&
      !element.closest("[inert]") &&
      element.getClientRects().length > 0,
  )
}

/**
 * Handles Tab in the pane grid: the next stop in the pane on screen after
 * this one, or before it with Shift, when the browser's order would go
 * elsewhere. `layout` is read when the key is pressed.
 */
export function paneTabOrder(layout: () => PaneLayout | null) {
  return (event: ReactKeyboardEvent<HTMLElement>) => {
    if (event.key !== "Tab" || event.altKey || event.metaKey || event.ctrlKey) return
    const grid = event.currentTarget
    const from = (event.target as Element).closest<HTMLElement>("[data-pane-key]")
    const panes = layout()
    if (!from || !panes) return
    const here = stops(from)
    const at = here.indexOf(event.target as HTMLElement)
    const back = event.shiftKey
    // Within the pane, the browser's order is the one on screen.
    if (at < 0 || (back ? at > 0 : at < here.length - 1)) return
    const order = panesOf(panes).map((pane) => pane.key)
    const index = order.indexOf(Number(from.dataset.paneKey))
    const neighbour = order[index + (back ? -1 : 1)]
    const next =
      neighbour === undefined
        ? outside(grid, back)
        : (() => {
            const pane = grid.querySelector(`[data-pane-key="${neighbour}"]`)
            const there = pane ? stops(pane) : []
            return back ? there.at(-1) : there[0]
          })()
    if (!next) return
    event.preventDefault()
    next.focus()
  }
}

/** The first stop after the grid, or the last before it, in the page. */
function outside(grid: HTMLElement, back: boolean): HTMLElement | undefined {
  const all = stops(document)
  const outsideGrid = all.filter((element) => !grid.contains(element))
  const after = outsideGrid.filter(
    (element) =>
      grid.compareDocumentPosition(element) &
      (back ? Node.DOCUMENT_POSITION_PRECEDING : Node.DOCUMENT_POSITION_FOLLOWING),
  )
  return back ? after.at(-1) : after[0]
}
