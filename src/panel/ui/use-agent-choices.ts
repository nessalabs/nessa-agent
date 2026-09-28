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

/** Creation reads the stored draft choice, so bind the catalog choice shown by the panel. */
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
    if (!selection && !serverConversationId && agent && model) {
      setSelection(id, { agent, model, approvalMode: "ask" })
    }
  }, [id, selection, serverConversationId, agent, model, setSelection])
}
