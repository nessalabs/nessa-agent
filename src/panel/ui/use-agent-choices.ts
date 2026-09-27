import type { AgentsListResult } from "@nessa/client"
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
