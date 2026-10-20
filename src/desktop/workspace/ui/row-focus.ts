/**
 * Where focus goes when the row holding it goes: archived, deleted, here or
 * on another device, from a menu, a button, a swipe or a question. One rule
 * for every way a row leaves, owned by the list it leaves: the same session's
 * row if the list draws it again (its session moved group), else the row now
 * at its place, or the last when it was the last, so the list's keys keep
 * working.
 *
 * The list keeps focus itself while focus is in it (`useRowFocusKept`). A
 * menu or a question a row opened is drawn outside every list, so the row
 * notes its place as it opens one (`rowPlace`). As it closes, focus goes
 * back where it was; when that was the row or nothing, it goes by that place
 * (`focusRowPlace`): to the row, or past it when it went meanwhile, and only
 * ever in the list it was asked from. A dialog that outlives that menu, such
 * as Share, looks the session's row up when it closes (`focusSessionRow`).
 */
import { useEffect, type RefObject } from "react"

/** A row's place in the list that keeps it. */
export interface RowPlace {
  readonly list: HTMLElement
  /** The session it shows, when it shows one; any other row is its node. */
  readonly id: string | null
  readonly node: HTMLElement
  readonly at: number
}

const LIST = "data-row-list"

function rowsOf(list: HTMLElement): HTMLElement[] {
  return [...list.querySelectorAll<HTMLElement>(list.getAttribute(LIST) ?? "")]
}

/** The place of the row `node` is in, in the list that keeps it; `null` outside every kept list. */
export function rowPlace(node: Element | null): RowPlace | null {
  const list = node?.closest<HTMLElement>(`[${LIST}]`)
  if (!list || !node) return null
  const row =
    node.closest<HTMLElement>(list.getAttribute(LIST) ?? "") ?? beside(node, list)
  if (!row || !list.contains(row)) return null
  return {
    list,
    id: row.getAttribute("data-session-row"),
    node: row,
    at: rowsOf(list).indexOf(row),
  }
}

/**
 * The row a control drawn beside it belongs to, such as its swipe tray's
 * buttons: the one row in the nearest wrapper that holds any.
 */
function beside(node: Element, list: HTMLElement): HTMLElement | null {
  for (let at = node.parentElement; at && at !== list; at = at.parentElement) {
    const found = at.querySelectorAll<HTMLElement>(list.getAttribute(LIST) ?? "")
    if (found.length) return found.length === 1 ? found[0] : null
  }
  return null
}

/**
 * Focus the session row `id` names, as it is drawn now. In a list that keeps
 * focus, that list's rule (`focusRowPlace`); otherwise the row itself.
 * Whether a row took it.
 */
export function focusSessionRow(id: string, root: ParentNode = document): boolean {
  const row = root.querySelector<HTMLElement>(`[data-session-row="${CSS.escape(id)}"]`)
  if (!row) return false
  const place = rowPlace(row)
  if (place) return focusRowPlace(place)
  row.focus({ preventScroll: true })
  return row.ownerDocument.activeElement === row
}

/** Focus the row at `place` by the rule above; whether a row took it. */
export function focusRowPlace(place: RowPlace | null): boolean {
  if (!place?.list.isConnected) return false
  const rows = rowsOf(place.list)
  const same = place.node.isConnected
    ? place.node
    : rows.find(
        (row) => place.id !== null && row.getAttribute("data-session-row") === place.id,
      )
  const next = same ?? rows[Math.min(place.at, rows.length - 1)]
  if (!next) return false
  next.focus({ preventScroll: true })
  return true
}

/**
 * `list` keeps focus in itself when the focused row in it is removed, by the
 * rule above. `rows` selects its rows.
 */
export function useRowFocusKept(list: RefObject<HTMLElement | null>, rows: string): void {
  useEffect(() => {
    const element = list.current
    if (!element) return
    const ownerDocument = element.ownerDocument
    element.setAttribute(LIST, rows)
    // The row focus is in, and where it stood; let go once focus or the
    // pointer goes somewhere else of its own. Focus going nowhere is its
    // row being removed, which the list still owes a place.
    let held: RowPlace | null = null
    const onFocusIn = (event: FocusEvent) => {
      held = rowPlace(event.target as Element | null) ?? held
    }
    const onFocusOut = (event: FocusEvent) => {
      const to = event.relatedTarget as Node | null
      if (to !== null && !element.contains(to)) held = null
    }
    const onPointerDown = (event: PointerEvent) => {
      if (!element.contains(event.target as Node)) held = null
    }
    const observer = new MutationObserver(() => {
      if (!held) return
      // Still drawn: where it stands now, past rows gone above it.
      if (held.node.isConnected) {
        held = rowPlace(held.node)
        return
      }
      const place = held
      held = null
      const active = ownerDocument.activeElement
      if (active && active !== ownerDocument.body && active.isConnected) return
      focusRowPlace(place)
    })
    element.addEventListener("focusin", onFocusIn)
    element.addEventListener("focusout", onFocusOut)
    ownerDocument.addEventListener("pointerdown", onPointerDown, true)
    observer.observe(element, { childList: true, subtree: true })
    return () => {
      element.removeAttribute(LIST)
      element.removeEventListener("focusin", onFocusIn)
      element.removeEventListener("focusout", onFocusOut)
      ownerDocument.removeEventListener("pointerdown", onPointerDown, true)
      observer.disconnect()
    }
  }, [list, rows])
}
