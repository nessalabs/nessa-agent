/**
 * Where Chat context reads a chat's decisions. Asking the gateway is a
 * request leaving the process, so composition supplies the source
 * (`main.tsx`); tests supply their own, and a window with none shows no
 * Decisions section.
 */
import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useSyncExternalStore,
  type ReactNode,
} from "react"
import type { DecisionSource } from "../../application/ports"
import type { Decision } from "../../model/decisions"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectSession } from "../../adapters/store/selectors"

type Held = {
  readonly state: "reading" | "read" | "failed"
  readonly decisions?: readonly Decision[]
}

/**
 * One read of a chat's decisions shared by everything that shows them
 * (Chat context, the dialog, the transcript card's auto-pick line), so a
 * chat is asked once per change, not once per place it is drawn. A chat is
 * read when the first of them appears, when its ask changes, on retry, and
 * all at once when the person's decision settings change;
 * only the newest read for a chat may show what it got.
 */
class DecisionReads {
  private readonly held = new Map<string, Held>()
  private readonly asked = new Map<string, string | undefined>()
  private readonly watching = new Map<string, number>()
  private readonly latest = new Map<string, number>()
  private readonly listeners = new Set<() => void>()
  private reads = 0

  constructor(readonly source: DecisionSource) {}

  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    return () => {
      this.listeners.delete(listener)
    }
  }

  get(sessionId: string): Held | undefined {
    return this.held.get(sessionId)
  }

  /** Something shows `sessionId`'s decisions while its ask is `askId`; the returned function says it stopped. */
  watch(sessionId: string, askId: string | undefined): () => void {
    const count = this.watching.get(sessionId) ?? 0
    this.watching.set(sessionId, count + 1)
    if (count === 0 || this.asked.get(sessionId) !== askId) this.read(sessionId, askId)
    return () => {
      const left = (this.watching.get(sessionId) ?? 1) - 1
      if (left > 0) this.watching.set(sessionId, left)
      else this.watching.delete(sessionId)
    }
  }

  retry(sessionId: string) {
    this.read(sessionId, this.asked.get(sessionId))
  }

  /** Read every chat something shows again: what the person's settings decide changed. */
  refresh = () => {
    for (const sessionId of this.watching.keys()) this.retry(sessionId)
  }

  private read(sessionId: string, askId: string | undefined) {
    const current = ++this.reads
    this.latest.set(sessionId, current)
    this.asked.set(sessionId, askId)
    const last = this.held.get(sessionId)
    this.set(sessionId, {
      state: "reading",
      ...(last?.decisions ? { decisions: last.decisions } : {}),
    })
    this.source.decisions(sessionId).then(
      (decisions) => {
        if (this.latest.get(sessionId) === current)
          this.set(sessionId, { state: "read", decisions })
      },
      (error: unknown) => {
        console.error("Reading a chat's decisions failed", error)
        if (this.latest.get(sessionId) !== current) return
        const kept = this.held.get(sessionId)?.decisions
        this.set(sessionId, { state: "failed", ...(kept ? { decisions: kept } : {}) })
      },
    )
  }

  private set(sessionId: string, held: Held) {
    this.held.set(sessionId, held)
    this.listeners.forEach((listener) => listener())
  }
}

const DecisionsContext = createContext<DecisionReads | null>(null)

export function DecisionsProvider({
  source,
  children,
}: {
  source: DecisionSource | undefined
  children: ReactNode
}) {
  const reads = useMemo(() => (source ? new DecisionReads(source) : null), [source])
  return <DecisionsContext.Provider value={reads}>{children}</DecisionsContext.Provider>
}

const nothingToRefresh = () => {}

/**
 * Reads every shown chat's decisions again: for Settings to call once the
 * person's decision settings are saved, since they move every countdown.
 */
export function useRefreshDecisions(): () => void {
  return useContext(DecisionsContext)?.refresh ?? nothingToRefresh
}

/** The window's decisions source, for what a decision's controls change. */
export function useDecisionSource(): DecisionSource | null {
  return useContext(DecisionsContext)?.source ?? null
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

const unsubscribed = () => () => {}

/**
 * A chat's decisions, read again whenever its list row's ask changes (a new
 * ask replaces the last, and the person's answer settles it), and when
 * asked to retry. A read for another chat is never shown for this one.
 */
export function useDecisions(sessionId: string): DecisionsRead {
  const reads = useContext(DecisionsContext)
  const askId = useWorkspaceSelector(
    (state) => selectSession(state, sessionId)?.ask?.askId,
  )
  const held = useSyncExternalStore(reads?.subscribe ?? unsubscribed, () =>
    reads?.get(sessionId),
  )
  useEffect(() => reads?.watch(sessionId, askId), [reads, sessionId, askId])
  const retry = useMemo(() => () => reads?.retry(sessionId), [reads, sessionId])
  if (!reads) return { state: "none" }
  if (!held) return { state: "reading", retry }
  return {
    state: held.state,
    ...(held.decisions ? { decisions: held.decisions } : {}),
    retry,
  }
}
