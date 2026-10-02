/**
 * An app widget's tool call, read from its plugin's calls port as it
 * changes, and what the widget's answer (`useWidget`, ADR 326) is for it: not
 * read yet, gone, or ready — titled by its tool, belonging to the session the
 * call was made in.
 */
import { useCallback, useSyncExternalStore } from "react"
import type { WidgetState } from "../../model/widget-state"
import type { AppWidgetPlugin } from "../../ui/plugin"
import type { CallRead } from "../application/ports"

/** The call widget `id` names, following its changes. */
export function useAppCall(plugin: AppWidgetPlugin, id: string): CallRead {
  const { calls } = plugin.ports
  const subscribe = useCallback(
    (listener: () => void) => calls.subscribe(id, listener),
    [calls, id],
  )
  const read = useCallback(() => calls.read(id), [calls, id])
  return useSyncExternalStore(subscribe, read, read)
}

/** The widget's answer for its call. */
export function appWidgetState(call: CallRead): WidgetState {
  switch (call.kind) {
    case "unread":
      return { kind: "unread" }
    case "missing":
      return { kind: "missing" }
    case "known":
      return { kind: "ready", title: call.call.tool, origin: call.call.sessionId }
  }
}
