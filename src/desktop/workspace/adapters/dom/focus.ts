/**
 * Keyboard focus follows what is in front (ADR 238, amended by ADR 326): the
 * focused pane — its composer when it shows a session, its body when it
 * shows a widget, for the view to place further — or, while the window shows
 * a widget, that widget's body. Whatever changes which pane is focused, or
 * what it shows — a split, ⌘N, ⌘W, ⌘1–4, a session opened from ⌘K, an
 * agent's dispatch — the caret lands there once it is on the page. So does
 * focus that falls to the page because what held it went away: an answered
 * approval, a closed pane, a widget's detail closed — or was hidden by the
 * stylesheet. While the window shows a widget, focus that falls away lands in
 * the window's widget, never in a pane beneath it.
 *
 * When another pane takes focus, the caret goes with it from wherever it
 * was but a dialog or a menu. When only what the focused pane shows
 * changes, it moves focus that was lost, or left in another pane: a list
 * walked with the arrow keys, the sidebar, is the person's, and keeps it.
 * Opening a widget in the window moves focus into it, from anywhere but a
 * dialog or a menu.
 *
 * Coming back from the Agents overview, or from the window, is another pane
 * taking focus, even when the command that brought the panes back changed
 * nothing else.
 *
 * This is the one place focus follows the store; the pane's own focus
 * handler is the other direction (a click or Tab into a pane focuses it).
 */
import { useEffect, type RefObject } from "react"
import { gridOf } from "../../../split-panes"
import { focusedPane } from "../../../split-panes/model/pane-layout"
import { modalSelector } from "../../../adapters/modal"
import type { DesktopStore } from "../../../store"
import { widgetBodyAttribute } from "../../../widgets"
import { paneItemKey, widgetItem } from "../../model/pane-item"
import { selectContentKind, selectWindowWidget } from "../store/selectors"

/** Marks the focused pane, whichever layout draws it, for the caret to find. */
export const focusedPaneAttribute = "data-pane-focused"

/** Marks the window's widget layer while it shows one (`ui/panes/widget-window.tsx`). */
export const widgetWindowAttribute = "data-widget-window"

/**
 * Where the caret lands now: the window's widget body while the window shows
 * one, else the focused pane's composer (a session) or body (a widget), if
 * it is on the page.
 */
function caretTarget(scope: ParentNode): HTMLElement | null {
  return (
    scope.querySelector<HTMLElement>(
      `[${widgetWindowAttribute}] [${widgetBodyAttribute}]`,
    ) ??
    scope.querySelector<HTMLElement>(
      `[${focusedPaneAttribute}] .desktop-composer textarea, [${focusedPaneAttribute}] [${widgetBodyAttribute}]`,
    )
  )
}

/**
 * Puts the caret in what is in front (`caretTarget`), trying for a few frames
 * while a new pane fills in; `done` is told once it has landed, or given up.
 * Returns a function that stops trying.
 */
export function focusInFront(
  scope: ParentNode = document,
  done: () => void = () => {},
): () => void {
  let frame = 0
  let tries = 0
  const attempt = () => {
    const field = caretTarget(scope)
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
 * many there are, and what the content region shows — the panes, the
 * overview, or a widget, by its item's key.
 */
function signature(store: DesktopStore): {
  pane: string
  rest: string
  shown: string
} {
  const state = store.getState()
  const { panes } = state.workspace
  const widget = selectWindowWidget(state)
  const shown = widget ? paneItemKey(widgetItem(widget)) : selectContentKind(state)
  if (!panes) return { pane: "", rest: "", shown }
  const pane = focusedPane(panes)
  // What it shows by its key alone: two keys are equal exactly when their
  // items are (`model/pane-item.ts`), so nothing here reads what it names.
  return {
    pane: String(pane.key),
    rest: `${pane.item}:${panes.columns.map((column) => column.panes.length).join(",")}`,
    shown,
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
    // Where focus last was in the panes or the window, to tell focus that fell away from focus taken elsewhere.
    let last: Element | null = null

    const follow = () => {
      stop()
      stop = focusInFront(scope)
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
      if (!lost && active.closest(modalSelector)) return
      const inFocused = !lost && active.closest(`[${focusedPaneAttribute}]`) !== null
      const grid = gridOf(scope)
      const inOtherPane = !lost && !inFocused && grid?.contains(active) === true
      if (lost || inOtherPane || (moved && !inFocused)) follow()
    }
    /** A widget opened in the window: the caret goes into it, from anywhere but a dialog or a menu. */
    const intoWindow = () => {
      const active = document.activeElement
      if (active && active !== document.body && active.closest(modalSelector)) return
      follow()
    }

    let seen = signature(store)
    const unsubscribe = store.subscribe(() => {
      const next = signature(store)
      const was = seen
      seen = next
      // The Agents overview over the panes keeps the keyboard itself.
      if (next.shown === "agents") return
      // A widget in the window, opened or replaced by another.
      if (next.shown !== "panes") {
        if (next.shown !== was.shown) requestAnimationFrame(intoWindow)
        return
      }
      const back = was.shown !== "panes"
      if (!back && next.pane === was.pane && next.rest === was.rest) return
      // Another pane took focus, or the panes came back from under the
      // overview or the window — by a command that changed nothing else, too.
      const moved = back || next.pane !== was.pane
      // After the change reaches the page.
      requestAnimationFrame(() => settle(moved))
    })

    // Focus falls to the page when what held it is taken away, or hidden by
    // the stylesheet; the browser says so unevenly, so the panes' changes,
    // and the size of what holds focus, are watched instead.
    const fellAway = (away: (held: Element) => boolean) => () => {
      if (!last || !away(last)) return
      const active = document.activeElement
      // Hidden, it may still hold focus as this runs (WebKit does), before the browser lets go.
      if (active !== document.body && active !== last) return
      last = null
      follow()
    }
    const watcher = new MutationObserver(fellAway((held) => !held.isConnected))
    watcher.observe(scope, { childList: true, subtree: true })
    // A hidden element has no box.
    const hiding =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(fellAway((held) => held.getClientRects().length === 0))
    // Focus held in the panes, or in the window's widget, is watched for falling away.
    const watched = (target: Element) =>
      gridOf(scope)?.contains(target) === true ||
      scope.querySelector(`[${widgetWindowAttribute}]`)?.contains(target) === true
    const onFocusIn = (event: FocusEvent) => {
      const target = event.target as Element | null
      last = target && watched(target) ? target : null
      hiding?.disconnect()
      if (last) hiding?.observe(last)
    }
    scope.addEventListener("focusin", onFocusIn)
    return () => {
      stop()
      unsubscribe()
      watcher.disconnect()
      hiding?.disconnect()
      scope.removeEventListener("focusin", onFocusIn)
    }
  }, [store, root])
}
