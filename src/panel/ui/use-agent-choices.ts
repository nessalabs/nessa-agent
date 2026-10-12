import type { ConversationSelection } from "../../conversation"
import type {
  ConversationChoices,
  FollowConversationChoices,
} from "../adapters/agent-choices"
import * as React from "react"

/**
 * The choices the pickers offer, held while the gateway is available: the
 * catalog is followed, not read once, so a paired environment that connects
 * or goes while the panel is open is offered, or no longer, as it happens.
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
 * the panel. The catalog is what the gateway offers now: a draft that names a
 * host it no longer offers (`environments`, once the catalog is known) drops
 * that host and runs here, as "Run on" then shows, rather than keep a choice
 * nobody can see and every send would be refused for.
 */
export function useBindCatalogFallback({
  id,
  selection,
  serverConversationId,
  agent,
  model,
  environments,
  setSelection,
}: {
  id: string
  selection?: ConversationSelection
  serverConversationId?: string
  agent?: string
  model?: string
  environments?: readonly string[]
  setSelection: (id: string, selection: ConversationSelection) => void
}): void {
  React.useLayoutEffect(() => {
    if (serverConversationId) return
    if (!selection) {
      if (agent && model) setSelection(id, { agent, model, approvalMode: "ask" })
      return
    }
    if (
      selection.environment !== undefined &&
      environments &&
      !environments.includes(selection.environment)
    ) {
      const { environment: _unoffered, ...offered } = selection
      setSelection(id, offered)
    }
  }, [id, selection, serverConversationId, agent, model, environments, setSelection])
}
