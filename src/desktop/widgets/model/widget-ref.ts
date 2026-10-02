/**
 * A widget, by reference: the plugin that draws it and the plugin's own id
 * for the thing drawn (ADR 326). The one shape everywhere a widget is named —
 * a pane item, a transcript part, the window's content view, a command.
 */

export interface WidgetRef {
  readonly plugin: string
  readonly id: string
}

/** Whether two references name the same widget: by value, never by identity. */
export function sameWidget(a: WidgetRef, b: WidgetRef): boolean {
  return a.plugin === b.plugin && a.id === b.id
}
