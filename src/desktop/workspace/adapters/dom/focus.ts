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
 * dialog or a menu; so does opening one in the chat's right panel, into the
 * panel's tab in front.
 *
 * Coming back from the Agents overview, or from the window, is another pane
 * taking focus, even when the command that brought the panes back changed
 * nothing else.
 *
 * This is the one place focus follows the store; the pane's own focus
 * handler is the other direction (a click or Tab into a pane focuses it).
 */
import { useEffect, type RefObject } from "react"
import { gridOf, marks } from "../../../split-panes"
import { focusedPane } from "../../../split-panes/model/pane-layout"
import { inModal } from "../../../adapters/modal"
import type { DesktopStore } from "../../../store"
import { widgetBodyAttribute } from "../../../widgets"
import { paneItemKey, widgetItem } from "../../model/pane-item"
import {
  selectContentKind,
  selectFocusedPaneKey,
  selectWindowWidget,
} from "../store/selectors"
import { arrivalWaitingAttribute } from "./arrival"

/** Marks the focused pane, whichever layout draws it, for the caret to find. */
export const focusedPaneAttribute = "data-pane-focused"

/** Marks the window's widget layer while it shows one (`ui/panes/widget-window.tsx`). */
export const widgetWindowAttribute = "data-widget-window"

/** Marks the right panel's tab in front (`ui/chrome/panel-content.tsx`). */
export const panelWidgetAttribute = "data-panel-widget"

/** Marks the right panel itself, its tabs and the tab in front (`ui/chrome/right-panel.tsx`). */
export const chatPanelAttribute = "data-chat-panel"

/**
 * Let go of a home before its send mutates the page. A command not asked
 * may restore that same field; a replaced home or deliberate focus move
 * keeps its authority. The normal pane controller owns the new composer.
 */
export function retireHomeFocus(
  home: HTMLElement | null,
  current: () => boolean,
): () => void {
  const held = document.activeElement
  if (
    !home?.isConnected ||
    !current() ||
    !(held instanceof HTMLElement) ||
    !home.contains(held) ||
    inModal(held)
  )
    return () => {}
  held.blur()
  return () => {
    if (
      current() &&
      home.isConnected &&
      held.isConnected &&
      home.contains(held) &&
      !inModal(held) &&
      held.closest("[inert]") === null &&
      document.activeElement === document.body
    )
      held.focus({ preventScroll: true })
  }
}

/**
 * Where the caret lands now: the window's widget body while the window shows
 * one, else the focused pane's composer (a session) or body (a widget), if
 * it is on the page.
 */
function caretTarget(scope: ParentNode): HTMLElement | null {
  const windowWidget = scope.querySelector<HTMLElement>(`[${widgetWindowAttribute}]`)
  const region = windowWidget ?? scope
  const selector = windowWidget
    ? `[${widgetBodyAttribute}]`
    : `[${focusedPaneAttribute}] .desktop-composer textarea, [${focusedPaneAttribute}] [${widgetBodyAttribute}]`
  const candidates = [...region.querySelectorAll<HTMLElement>(selector)]
  return (
    candidates.find(
      (element) =>
        element.closest(
          `[inert], [${marks.reflow}], [${marks.flying}], [${marks.measuring}], [${arrivalWaitingAttribute}]`,
        ) === null,
    ) ?? null
  )
}

/** The body of the right panel's tab in front, if it is on the page and not covered. */
function panelTarget(scope: ParentNode): HTMLElement | null {
  const body = scope.querySelector<HTMLElement>(
    `[${panelWidgetAttribute}] [${widgetBodyAttribute}]`,
  )
  return body && body.closest("[inert]") === null ? body : null
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
  return focusAfterPaint(() => caretTarget(scope), done)
}

/** Resolves a caller-owned target, waits for its paint, then rechecks focus authority. */
export function focusAfterPaint(
  target: () => HTMLElement | null,
  done: () => void = () => {},
): () => void {
  let frame = 0
  let tries = 0
  let active = true
  const held = document.activeElement
  const attempt = () => {
    if (!active) return
    if (tries++ >= 30) return done()
    const field = target()
    if (!field) {
      frame = requestAnimationFrame(attempt)
      return
    }
    // A pane may have filled on this frame. Let its layout paint before focus
    // asks the browser for geometry; recheck the target after that paint.
    frame = requestAnimationFrame(() => {
      if (!active) return
      const focus = document.activeElement
      if (inModal(focus) || field.closest("[inert]")) return done()
      // A deliberate focus move during the wait belongs to the person.
      if (focus !== held && focus !== document.body && focus !== field) return done()
      if (target() !== field) return attempt()
      field.focus({ preventScroll: true })
      done()
    })
  }
  frame = requestAnimationFrame(attempt)
  return () => {
    active = false
    cancelAnimationFrame(frame)
  }
}

/**
 * What moving the caret depends on: which pane is focused, what it shows, how
 * many there are, what the content region shows — the panes, the overview,
 * or a widget, by its item's key — and each widget opened in a panel.
 */
function signature(store: DesktopStore): {
  pane: string
  rest: string
  shown: string
  panel: number
} {
  const state = store.getState()
  const { panes, panelAsked: panel } = state.workspace
  const widget = selectWindowWidget(state)
  const shown = widget ? paneItemKey(widgetItem(widget)) : selectContentKind(state)
  if (!panes) return { pane: "", rest: "", shown, panel }
  const pane = focusedPane(panes)
  // What it shows by its key alone: two keys are equal exactly when their
  // items are (`model/pane-item.ts`), so nothing here reads what it names.
  return {
    pane: String(pane.key),
    rest: `${pane.item}:${panes.columns.map((column) => column.panes.length).join(",")}`,
    shown,
    panel,
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
    let active = true
    const frames = new Set<number>()
    // Settlement belongs to this effect, including the second overview frame.
    const afterPaint = (action: () => void) => {
      const frame = requestAnimationFrame(() => {
        frames.delete(frame)
        if (active) action()
      })
      frames.add(frame)
    }
    // Where focus last was in the panes or the window, to tell focus that fell away from focus taken elsewhere.
    let last: Element | null = null
    let lastInPanel = false

    const follow = () => {
      stop()
      // The focused mark is written on the frame after the click. Until it
      // names the pane the store focused, there is nothing to land in.
      stop = focusAfterPaint(() => {
        const key = selectFocusedPaneKey(store.getState())
        const marked = scope.querySelector<HTMLElement>(`[${focusedPaneAttribute}]`)
        if (key !== null && marked?.getAttribute("data-pane-key") !== String(key))
          return null
        return caretTarget(scope)
      })
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
      if (inModal(active)) return
      // Document tabs own their arrow-key navigation while the selected tab
      // still holds focus. A shortcut that creates a new document deselects
      // the old tab; its caret must then follow the new focused pane.
      if (active?.closest('[role="tablist"]')) {
        if (active.closest('[role="tab"]')?.getAttribute("aria-selected") === "true")
          return
        follow()
        return
      }
      const key = selectFocusedPaneKey(store.getState())
      const marked = scope.querySelector<HTMLElement>(`[${focusedPaneAttribute}]`)
      const markAgrees =
        key === null || marked?.getAttribute("data-pane-key") === String(key)
      const inFocused =
        markAgrees && !lost && active.closest(`[${focusedPaneAttribute}]`) !== null
      const grid = gridOf(scope)
      const inOtherPane = !lost && !inFocused && grid?.contains(active) === true
      if (lost || inOtherPane || (moved && !inFocused)) follow()
    }
    /** A widget opened in the window: the caret goes into it, from anywhere but a dialog or a menu. */
    const intoWindow = () => {
      const active = document.activeElement
      if (inModal(active)) return
      follow()
    }

    /** A widget opened in the panel: the caret goes into its tab, from anywhere but a dialog or a menu. */
    const intoPanel = () => {
      if (inModal(document.activeElement)) return
      stop()
      stop = focusAfterPaint(() => panelTarget(scope))
    }

    let seen = signature(store)
    const unsubscribe = store.subscribe(() => {
      const next = signature(store)
      const was = seen
      seen = next
      // Opening it in the panel brought its chat to the front, too: the
      // caret goes to the widget, not the chat's composer.
      if (next.panel !== was.panel) {
        afterPaint(intoPanel)
        return
      }
      // The Agents overview over the panes keeps the keyboard itself.
      if (next.shown === "agents") return
      // A widget in the window, opened or replaced by another.
      if (next.shown !== "panes") {
        if (next.shown !== was.shown) afterPaint(intoWindow)
        return
      }
      const back = was.shown !== "panes"
      if (!back && next.pane === was.pane && next.rest === was.rest) return
      // Another pane took focus, or the panes came back from under the
      // overview or the window — by a command that changed nothing else, too.
      const moved = back || next.pane !== was.pane
      // After the change reaches the page. Coming back from the overview,
      // one frame later still: the leave's own frame only lifts the cover
      // (`overview-layer.tsx`).
      if (back) afterPaint(() => afterPaint(() => settle(moved)))
      else afterPaint(() => settle(moved))
    })

    // Focus falls to the page when what held it is taken away, or hidden by
    // the stylesheet; the browser says so unevenly, so the panes' changes,
    // and the size of what holds focus, are watched instead.
    const fellAway = (away: (held: Element) => boolean) => () => {
      if (!last || !away(last)) return
      const active = document.activeElement
      // Hidden, it may still hold focus as this runs (WebKit does), before the browser lets go.
      if (active !== document.body && active !== last) return
      // Out of the panel — a tab closed, a detail gone — back into its tab
      // in front while it shows one, as a pane keeps the caret in its own
      // body; else to the focused pane.
      // Read as focus came in: taken away, it is in no panel any more.
      const inPanel = lastInPanel
      last = null
      if (!inPanel) return follow()
      stop()
      stop = focusAfterPaint(() => panelTarget(scope) ?? caretTarget(scope))
    }
    const watcher = new MutationObserver(fellAway((held) => !held.isConnected))
    watcher.observe(scope, { childList: true, subtree: true })
    // A hidden element has no box.
    const hiding =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(fellAway((held) => held.getClientRects().length === 0))
    // Focus held in the panes, the window's widget or the panel, is watched for falling away.
    const watched = (target: Element) =>
      gridOf(scope)?.contains(target) === true ||
      scope.querySelector(`[${widgetWindowAttribute}]`)?.contains(target) === true ||
      target.closest(`[${chatPanelAttribute}]`) !== null
    const onFocusIn = (event: FocusEvent) => {
      const target = event.target as Element | null
      last = target && watched(target) ? target : null
      lastInPanel = last?.closest(`[${chatPanelAttribute}]`) != null
      hiding?.disconnect()
      if (last) hiding?.observe(last)
    }
    scope.addEventListener("focusin", onFocusIn)
    return () => {
      active = false
      for (const frame of frames) cancelAnimationFrame(frame)
      frames.clear()
      stop()
      unsubscribe()
      watcher.disconnect()
      hiding?.disconnect()
      scope.removeEventListener("focusin", onFocusIn)
    }
  }, [store, root])
}
