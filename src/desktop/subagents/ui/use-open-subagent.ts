/**
 * Opens a conversation's panel on one of its subagents. The choice is this
 * vertical's; other verticals call this and do not read that map.
 */
import { useCallback } from "react"
import type { OpenPlace } from "../../widgets/model/widget-state"
import type { WidgetHost } from "../../widgets/ui/plugin"
import { joinedSubagentId, subagentsPluginId } from "../application/ports"
import { chooseSubagent } from "../application/selection"

export function useOpenSubagent(host: WidgetHost, place: OpenPlace) {
  return useCallback(
    (target: { sessionId: string; sourceKey: string; sourceId: string }) => {
      chooseSubagent(
        target.sessionId,
        joinedSubagentId(target.sourceKey, target.sourceId),
      )
      host.openWidget({ plugin: subagentsPluginId, id: target.sessionId }, place)
    },
    [host, place],
  )
}
