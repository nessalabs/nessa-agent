/**
 * The sample plugin's widgets: one for each answer a plugin can give, and one
 * for a plugin the window does not have, so every row of ADR 326's table can
 * be seen before any real plugin lands. The sample workspace's conversation
 * that carries them (`workspace/adapters/in-memory/`) names them by these
 * references; composition says which session they belong to.
 */
import type { WidgetRef } from "../model/widget-ref"
import type { WidgetState } from "../model/widget-state"

/** The sample plugin's id. */
export const samplePluginId = "sample"

const ref = (id: string): WidgetRef => ({ plugin: samplePluginId, id })

export const sampleWidgets = {
  /** Ready, with a trail: a detail it opens and closes, Escape stepping back out of it. */
  trail: ref("trail"),
  /** Ready: what the trail opens beside itself. */
  notes: ref("notes"),
  unread: ref("unread"),
  missing: ref("missing"),
  off: ref("off"),
  unshowable: ref("unshowable"),
  /** A plugin no window registers. */
  unregistered: { plugin: "not-registered", id: "anything" },
} as const satisfies Record<string, WidgetRef>

/** What the sample plugin answers for `id`, its ready widgets belonging to `origin`. */
export function sampleState(id: string, origin: string): WidgetState {
  switch (id) {
    case sampleWidgets.trail.id:
      return { kind: "ready", title: "Sample trail", origin }
    case sampleWidgets.notes.id:
      return { kind: "ready", title: "Sample notes", origin }
    case sampleWidgets.unread.id:
      return { kind: "unread" }
    case sampleWidgets.off.id:
      return { kind: "off" }
    case sampleWidgets.unshowable.id:
      return { kind: "unshowable" }
    default:
      return { kind: "missing" }
  }
}
