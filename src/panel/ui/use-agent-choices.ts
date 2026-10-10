import type { AgentsListResult } from "@nessa/client"
import type { ConversationSelection } from "../../conversation"
import * as React from "react"

export type ConversationChoices = {
  catalog: AgentsListResult
  chosenAgent: string | undefined
}

export function useConversationChoices(
  available: boolean,
  load: () => Promise<ConversationChoices>,
): ConversationChoices | null {
  const [choices, setChoices] = React.useState<ConversationChoices | null>(null)
  React.useEffect(() => {
    if (!available) {
      setChoices(null)
      return
    }
    let current = true
    void load()
      .then((value) => {
        if (current) setChoices(value)
      })
      .catch(() => {
        if (current) setChoices(null)
      })
    return () => {
      current = false
    }
  }, [available, load])
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
