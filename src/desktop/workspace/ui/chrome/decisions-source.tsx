/**
 * Where Chat context reads a chat's decisions. Asking the gateway is a
 * request leaving the process, so composition supplies the source
 * (`main.tsx`); tests supply their own, and a window with none shows no
 * Decisions section.
 */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react"
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

/**
 * Where a chat's decisions stand. `decisions` is the last list read for
 * this chat, kept while the next read is on its way and after one fails, so
 * the rows and an open dialog stay put; `retry` asks again.
 */
export type DecisionsRead =
  | { readonly state: "none" }
  | {
      readonly state: "reading" | "read" | "failed"
      readonly decisions?: readonly Decision[]
      readonly retry: () => void
    }

type Held = {
  readonly sessionId: string
  readonly state: "reading" | "read" | "failed"
  readonly decisions?: readonly Decision[]
}

/**
 * A chat's decisions, read again whenever its list row's ask changes (a new
 * ask replaces the last, and the person's answer settles it), and when
 * asked to retry. A read for another chat is never shown for this one.
 */
export function useDecisions(sessionId: string): DecisionsRead {
  const source = useContext(DecisionsContext)
  const askId = useWorkspaceSelector(
    (state) => selectSession(state, sessionId)?.ask?.askId,
  )
  const [attempt, setAttempt] = useState(0)
  const [held, setHeld] = useState<Held | null>(null)
  useEffect(() => {
    if (!source) return
    let current = true
    setHeld((last) =>
      last?.sessionId === sessionId
        ? { ...last, state: "reading" }
        : { sessionId, state: "reading" },
    )
    source.decisions(sessionId).then(
      (decisions) => {
        if (current) setHeld({ sessionId, state: "read", decisions })
      },
      (error: unknown) => {
        console.error("Reading a chat's decisions failed", error)
        if (current)
          setHeld((last) => ({
            sessionId,
            state: "failed",
            ...(last?.sessionId === sessionId && last.decisions
              ? { decisions: last.decisions }
              : {}),
          }))
      },
    )
    return () => {
      current = false
    }
  }, [source, sessionId, askId, attempt])
  const retry = useCallback(() => setAttempt((count) => count + 1), [])
  if (!source) return { state: "none" }
  if (held?.sessionId !== sessionId) return { state: "reading", retry }
  return {
    state: held.state,
    ...(held.decisions ? { decisions: held.decisions } : {}),
    retry,
  }
}
