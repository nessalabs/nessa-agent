/**
 * What a pane shows. The split panes know an item only as an opaque string;
 * here it is a session (listed or a draft), or a widget a conversation
 * carries — an experiment — opened in a pane of its own, written
 * `widget:<plugin>:<id>` so it can never be mistaken for a session's id.
 */

export interface WidgetItem {
  readonly plugin: string
  readonly id: string
}

const prefix = "widget:"

/** The pane item that shows `widget`. */
export function widgetItem(widget: WidgetItem): string {
  return `${prefix}${widget.plugin}:${widget.id}`
}

/** The widget a pane item shows, or null for a session. */
export function widgetOfItem(item: string): WidgetItem | null {
  if (!item.startsWith(prefix)) return null
  const rest = item.slice(prefix.length)
  const split = rest.indexOf(":")
  if (split <= 0 || split === rest.length - 1) return null
  return { plugin: rest.slice(0, split), id: rest.slice(split + 1) }
}
