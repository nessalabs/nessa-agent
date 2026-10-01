/**
 * What a pane shows: a session (listed, or a new one not started), or a
 * widget (ADR 326). Split panes know an item only as an opaque string, and
 * treat two panes with one string as the same pane (`paneShowing`), so this
 * is the one codec between the two:
 *
 * ```text
 *   { kind: "session", sessionId }        ──▶  "s:" + sessionId
 *   { kind: "widget", widget: { plugin, id } } ──▶  "w:" + encodeId(plugin) + ":" + encodeId(id)
 * ```
 *
 * The prefix tells the kinds apart, and an encoded part never holds a `:`
 * (`model/id-encoding.ts`), so the encoding is one-to-one — two items share a
 * key exactly when they are equal — and canonical: `paneItemOf` reads back
 * only a key `paneItemKey` writes (`pane-item.test.ts`).
 *
 * The key is a branded type only `paneItemKey` makes; reading one back is
 * `paneItemOf`, the only thing here that takes a key. Split panes take any
 * string, so the type alone cannot keep a hand-built one out of a layout:
 * the workspace's tests read every pane they check through `paneItemOf` and
 * refuse a key it did not write (`shownBy` in `testing.ts`).
 */
import { decodeId, encodeId } from "../../model/id-encoding"
import type { WidgetRef } from "../../widgets/model/widget-ref"

export type PaneItem =
  | { readonly kind: "session"; readonly sessionId: string }
  | { readonly kind: "widget"; readonly widget: WidgetRef }

declare const paneItemBrand: unique symbol
/** A pane item as split panes hold it; made only by `paneItemKey`. */
export type PaneItemKey = string & { readonly [paneItemBrand]: true }

const sessionPrefix = "s:"
const widgetPrefix = "w:"

/** The item showing a session, listed or new. */
export function sessionItem(sessionId: string): PaneItem {
  return { kind: "session", sessionId }
}

/** The item showing a widget. */
export function widgetItem(widget: WidgetRef): PaneItem {
  return { kind: "widget", widget }
}

/** The key split panes hold for `item`. */
export function paneItemKey(item: PaneItem): PaneItemKey {
  const key =
    item.kind === "session"
      ? `${sessionPrefix}${item.sessionId}`
      : `${widgetPrefix}${encodeId(item.widget.plugin)}:${encodeId(item.widget.id)}`
  return key as PaneItemKey
}

/**
 * A key no item has: neither prefix, so `paneItemKey` never writes it and no
 * pane can hold it. What asks only whether the room fits one more pane uses
 * it, so a real item with any plugin and id can never answer for the room.
 */
export const roomProbeKey = "?" as PaneItemKey

/**
 * The item a pane's key names, or `null` for a string `paneItemKey` did not
 * write — which no pane holds, but a drag may carry from outside the grid.
 */
export function paneItemOf(key: string): PaneItem | null {
  if (key.startsWith(sessionPrefix)) return sessionItem(key.slice(sessionPrefix.length))
  if (!key.startsWith(widgetPrefix)) return null
  const parts = key.slice(widgetPrefix.length).split(":")
  if (parts.length !== 2) return null
  const plugin = decodeId(parts[0])
  const id = decodeId(parts[1])
  return plugin === null || id === null ? null : widgetItem({ plugin, id })
}
