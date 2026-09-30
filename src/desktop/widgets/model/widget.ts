/**
 * A widget: something a conversation carries besides its words — an
 * experiment, later a file, a browser, a chart. The transcript holds only a
 * reference; the plugin named in it decides what the reference shows, inline
 * in the conversation and opened beside it.
 */
export interface WidgetRef {
  /** Which plugin draws it: `"experiment"`. */
  readonly plugin: string
  /** What the plugin shows, in the plugin's own terms. */
  readonly id: string
}

export const sameWidget = (a: WidgetRef | null, b: WidgetRef | null): boolean =>
  a !== null && b !== null && a.plugin === b.plugin && a.id === b.id
