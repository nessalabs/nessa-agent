/**
 * Escape for the widget in front: the last rows of ADR 326's table of who
 * takes Escape, in order (_Focus_ and _Escape_, `docs/adr/todo/326-widgets.md`).
 * The owners before these — a menu or dialog, a carrying drag, the edge peek,
 * the search clearing its query — mark the event as theirs
 * (`defaultPrevented`) or stop it, and Settings makes the window inert; the
 * open overview is neither — no widget is in the window under it, and focus
 * is in no pane — so its Escape is left to it. Then the window's widget,
 * from anywhere — its view's last step back, else back to the panes — or,
 * with focus inside a widget's pane, that view's last step back, else
 * nothing: a widget pane is never closed by Escape, as a session pane is not.
 *
 * A host registers its view's steps back under its scope while it is on the
 * page (`useEscapeScope`): the window's, or a widget pane's, with the
 * element focus must be inside for a pane's to answer.
 */
import { createContext, useContext, useEffect, useMemo, type RefObject } from "react"
import type { DesktopStore } from "../../../store"
import { escapeStack, type EscapeStack } from "../../../widgets"
import { inModal } from "../../../adapters/modal"
import { showContent } from "../store/commands"

/** A host's Escape: its view's steps back, and the element it is drawn in. */
interface EscapeHost {
  readonly stack: EscapeStack
  readonly element: RefObject<HTMLElement | null>
}

/** Every host on the page, by scope: `window`, or a widget pane's own. */
export type EscapeScopes = Map<string, EscapeHost>

const EscapeScopesContext = createContext<EscapeScopes | null>(null)
export const EscapeScopesProvider = EscapeScopesContext.Provider

/** The window's scope: one widget at a time is shown there. */
export const windowScope = "window"

/** A widget pane's scope, by its pane's key. */
export const paneScope = (pane: number): string => `pane:${pane}`

/**
 * The steps back for a host drawn in `element`, registered under `scope`
 * while it is on the page. A view's `onEscape` pushes onto it.
 */
export function useEscapeScope(
  scope: string,
  element: RefObject<HTMLElement | null>,
): EscapeStack {
  const scopes = useContext(EscapeScopesContext)
  const stack = useMemo(() => escapeStack(), [])
  useEffect(() => {
    if (!scopes) return
    const host = { stack, element }
    scopes.set(scope, host)
    return () => {
      if (scopes.get(scope) === host) scopes.delete(scope)
    }
  }, [scopes, scope, stack, element])
  return stack
}

/** Runs the table above for the window's keys, from `root` down. */
export function useWidgetEscape({
  store,
  root,
  scopes,
}: {
  store: DesktopStore
  root: RefObject<HTMLElement | null>
  scopes: EscapeScopes
}): void {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      // An Escape that ends a composition is the field's.
      if (event.key !== "Escape" || event.defaultPrevented || event.isComposing) return
      const scope = root.current
      if (!scope || scope.closest("[inert]") || inModal(event.target)) return
      const { content } = store.getState().workspace
      if (typeof content === "object") {
        event.preventDefault()
        if (!scopes.get(windowScope)?.stack.escape())
          store.dispatch(showContent({ content: "panes" }))
        return
      }
      const active = document.activeElement
      if (!active) return
      for (const [name, host] of scopes) {
        if (name === windowScope || !host.element.current?.contains(active)) continue
        if (host.stack.escape()) event.preventDefault()
        return
      }
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [store, root, scopes])
}
