import type { ConversationSelection } from "../../conversation"
import type {
  ConversationChoices,
  FollowConversationChoices,
} from "../adapters/agent-choices"
import * as React from "react"

/**
 * The choices the pickers offer, held while the gateway is available: the
 * catalog is followed, not read once, so an agent that is installed or
 * removed while the panel is open is offered, or no longer, as it happens.
 * Nothing is offered until the first catalog arrives; the follow is stopped
 * when the gateway goes or the panel unmounts.
 */
export function useConversationChoices(
  available: boolean,
  follow: FollowConversationChoices,
): ConversationChoices | null {
  const [choices, setChoices] = React.useState<ConversationChoices | null>(null)
  React.useEffect(() => {
    if (!available) {
      setChoices(null)
      return
    }
    return follow(setChoices)
  }, [available, follow])
  return choices
}

/**
 * Creation reads the stored draft choice, so bind the catalog choice shown by
 * the panel to a draft that has none yet.
 */
export function useBindCatalogFallback({
  id,
  selection,
  serverConversationId,
  agent,
  model,
  setSelection,
}: {
  id: string
  selection?: ConversationSelection
  serverConversationId?: string
  agent?: string
  model?: string
  setSelection: (id: string, selection: ConversationSelection) => void
}): void {
  React.useLayoutEffect(() => {
    if (serverConversationId || selection) return
    if (agent && model) setSelection(id, { agent, model, approvalMode: "ask" })
  }, [id, selection, serverConversationId, agent, model, setSelection])
}
