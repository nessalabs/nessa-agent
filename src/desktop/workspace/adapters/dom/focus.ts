/**
 * Keyboard focus follows the focused pane. Whatever changes which pane is
 * focused, or what it shows — a split, ⌘N, ⌘W, ⌘1–4, a session opened from
 * ⌘K, an agent's dispatch — the caret lands in that pane's composer once it
 * is on the page. So does focus that falls to the page because what held it
 * went away: an answered approval, a closed pane.
 *
 * When another pane takes focus, the caret goes with it from wherever it
 * was but a dialog or a menu. When only what the focused pane shows
 * changes, it moves focus that was lost, or left in another pane: a list
 * walked with the arrow keys, the sidebar, is the person's, and keeps it.
 *
 * Coming back from the Agents overview is another pane taking focus, even
 * when the command that brought the panes back changed nothing else.
 *
 * This is the one place focus follows the store; the pane's own focus
 * handler is the other direction (a click or Tab into a pane focuses it).
 */
import { useEffect, type RefObject } from "react"
import { gridOf } from "../../../split-panes"
import { focusedPane } from "../../../split-panes/model/pane-layout"
import type { DesktopStore } from "../../../store"

/** Marks the focused pane, whichever layout draws it, for the caret to find. */
export const focusedPaneAttribute = "data-pane-focused"

/** The focused pane's composer field, if it is on the page. */
function composerField(scope: ParentNode): HTMLElement | null {
  return scope.querySelector<HTMLElement>(
    `[${focusedPaneAttribute}] .desktop-composer textarea`,
  )
}

/**
 * Puts the caret in the focused pane's composer, trying for a few frames
 * while a new pane fills in; `done` is told once it has landed, or given up.
 * Returns a function that stops trying.
 */
export function focusComposer(
  scope: ParentNode = document,
  done: () => void = () => {},
): () => void {
  let frame = 0
  let tries = 0
  const attempt = () => {
    const field = composerField(scope)
    if (field) {
      field.focus({ preventScroll: true })
      done()
    } else if (tries++ < 30) frame = requestAnimationFrame(attempt)
    else done()
  }
  frame = requestAnimationFrame(attempt)
  return () => cancelAnimationFrame(frame)
}

/**
 * What moving the caret depends on: which pane is focused, what it shows, how
 * many there are, and whether the panes are what the content region shows.
 */
function signature(store: DesktopStore): {
  pane: string
  rest: string
  panesShown: boolean
} {
  const { panes, content } = store.getState().workspace
  const panesShown = content === "panes"
  if (!panes) return { pane: "", rest: "", panesShown }
  const pane = focusedPane(panes)
  return {
    pane: String(pane.key),
    rest: `${pane.item}:${panes.columns.map((column) => column.panes.length).join(",")}`,
    panesShown,
  }
}

export function useFocusFollowsPane(
  store: DesktopStore,
  root: RefObject<HTMLElement | null>,
): void {
  useEffect(() => {
    const scope = root.current
    if (!scope) return
    let stop = () => {}
    // Where focus last was in the panes, to tell focus that fell away from focus taken elsewhere.
    let last: Element | null = null

    const follow = () => {
      stop()
      stop = focusComposer(scope)
    }
    /**
     * `moved`: another pane took focus — a key, a split, an agent — so the
     * caret goes with it from wherever it was but a dialog or a menu. Else
     * only focus that was lost, or left in another pane, follows: a list
     * walked with the arrow keys keeps its caret while its pane changes.
     */
    const settle = (moved: boolean) => {
      const active = document.activeElement
      const lost = !active || active === document.body
      if (!lost && active.closest('[role="dialog"], [role="menu"], [aria-modal="true"]'))
        return
      const inFocused = !lost && active.closest(`[${focusedPaneAttribute}]`) !== null
      const grid = gridOf(scope)
      const inOtherPane = !lost && !inFocused && grid?.contains(active) === true
      if (lost || inOtherPane || (moved && !inFocused)) follow()
    }

    let seen = signature(store)
    const unsubscribe = store.subscribe(() => {
      const next = signature(store)
      const was = seen
      seen = next
      // The Agents overview over the panes keeps the keyboard itself.
      if (!next.panesShown) return
      const back = !was.panesShown
      if (!back && next.pane === was.pane && next.rest === was.rest) return
      // Another pane took focus, or the panes came back from under the
      // overview — by a command that changed nothing else, too.
      const moved = back || next.pane !== was.pane
      // After the change reaches the page.
      requestAnimationFrame(() => settle(moved))
    })

    const onFocusIn = (event: FocusEvent) => {
      const target = event.target as Element | null
      last = target && gridOf(scope)?.contains(target) ? target : null
    }
    // Focus falls to the page when what held it is taken away; the browser says so
    // unevenly, so the panes' changes are watched instead.
    const watcher = new MutationObserver(() => {
      if (last && !last.isConnected && document.activeElement === document.body) {
        last = null
        follow()
      }
    })
    watcher.observe(scope, { childList: true, subtree: true })
    scope.addEventListener("focusin", onFocusIn)
    return () => {
      stop()
      unsubscribe()
      watcher.disconnect()
      scope.removeEventListener("focusin", onFocusIn)
    }
  }, [store, root])
}
