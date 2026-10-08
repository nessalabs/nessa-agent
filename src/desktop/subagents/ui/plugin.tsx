/**
 * The subagents widget: one per conversation, drawn in a pane and in the
 * window. What it answers is `subagentsWidgetState`, in that order. It has
 * no card of its own — a widget-only message is the host's title and Open.
 * Its header accessory is the conversation's subagents.
 */
import { useMemo, useSyncExternalStore } from "react"
import { useSubagentsPreview } from "../../adapters/window-preferences"
import type { WidgetState } from "../../widgets/model/widget-state"
import type { NativeWidgetPlugin, WidgetViewProps } from "../../widgets/ui/plugin"
import { selectSessionListing, useWorkspaceSelector } from "../../workspace"
import { useSubagentSource } from "../adapters/react/source-context"
import { subagentsPluginId } from "../application/ports"
import { subagentsWidgetState } from "../application/widget-state"
import { SubagentStackAccessory } from "./subagent-stack"
import { SubagentsPanel } from "./subagents-panel"

function SubagentsPane({ id, host }: WidgetViewProps) {
  return <SubagentsPanel sessionId={id} host={host} />
}

/** The panel, registered by composition beside the sample workspace. */
export function subagentsPlugin(): NativeWidgetPlugin {
  return {
    kind: "native",
    id: subagentsPluginId,
    name: "Subagents",
    useWidget: function useSubagentsWidget(id: string): WidgetState {
      const [preview] = useSubagentsPreview()
      const listing = useWorkspaceSelector((state) => selectSessionListing(state, id))
      const source = useSubagentSource()
      const read = useSyncExternalStore(
        (listener) => source.subscribe(listener),
        () => source.forSession(id),
      )
      return useMemo(
        () => subagentsWidgetState({ sessionId: id, preview, listing, read }),
        [id, preview, listing, read],
      )
    },
    views: { pane: SubagentsPane, window: SubagentsPane },
    SessionAccessory: SubagentStackAccessory,
  }
}
