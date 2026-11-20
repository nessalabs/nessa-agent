/**
 * Where Chat context reads a chat's decisions. Asking the gateway is a
 * request leaving the process, so composition supplies the source
 * (`main.tsx`); tests supply their own, and a window with none shows no
 * Decisions section.
 */
import { createContext, useContext, useEffect, useState, type ReactNode } from "react"
import type { DecisionSource } from "../../application/ports"
import type { Decision } from "../../model/decisions"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectSession } from "../../adapters/store/selectors"

const DecisionsContext = createContext<DecisionSource | null>(null)

export function DecisionsProvider({
  source,
  children,
}: {
  source: DecisionSource | undefined
  children: ReactNode
}) {
  return (
    <DecisionsContext.Provider value={source ?? null}>
      {children}
    </DecisionsContext.Provider>
  )
}

export type DecisionsRead =
  | { readonly state: "none" }
  | { readonly state: "reading"; readonly decisions?: readonly Decision[] }
  | { readonly state: "read"; readonly decisions: readonly Decision[] }
  | { readonly state: "failed" }

/**
 * A chat's decisions, read again whenever its list row's ask changes: a new
 * ask replaces the last, and the person's answer settles it. What was read
 * last stays shown while the next read is on its way.
 */
export function useDecisions(sessionId: string): DecisionsRead {
  const source = useContext(DecisionsContext)
  const askId = useWorkspaceSelector(
    (state) => selectSession(state, sessionId)?.ask?.askId,
  )
  const [read, setRead] = useState<DecisionsRead & { sessionId?: string }>({
    state: "none",
  })
  useEffect(() => {
    if (!source) return
    let current = true
    setRead((last) =>
      last.state === "read" && last.sessionId === sessionId
        ? { state: "reading", decisions: last.decisions, sessionId }
        : { state: "reading", sessionId },
    )
    source.decisions(sessionId).then(
      (decisions) => {
        if (current) setRead({ state: "read", decisions, sessionId })
      },
      (error: unknown) => {
        console.error("Reading a chat's decisions failed", error)
        if (current) setRead({ state: "failed", sessionId })
      },
    )
    return () => {
      current = false
    }
  }, [source, sessionId, askId])
  if (!source) return { state: "none" }
  // A read for another chat is never shown for this one.
  return read.sessionId === sessionId ? read : { state: "reading" }
}
