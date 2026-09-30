import {
  createContext,
  useCallback,
  useContext,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react"
import type { SubagentSource } from "../../application/ports"
import type { SessionSubagents } from "../../model/subagent"

interface SubagentsValue {
  readonly source: SubagentSource
  /** The subagent a panel was asked to show, by conversation. */
  readonly focused: ReadonlyMap<string, string>
  focus(sessionId: string, subagentId: string | null): void
}

const SubagentsContext = createContext<SubagentsValue | null>(null)

/**
 * The window's subagent source, from composition (`src/desktop/main.tsx`),
 * and which subagent each conversation's panel is showing — held here, above
 * every pane, so a click in one pane can open another pane on that subagent.
 */
export function SubagentsProvider({
  source,
  children,
}: {
  source: SubagentSource
  children: ReactNode
}) {
  const [focused, setFocused] = useState<ReadonlyMap<string, string>>(new Map())
  const focus = useCallback((sessionId: string, subagentId: string | null) => {
    setFocused((current) => {
      const next = new Map(current)
      if (subagentId === null) next.delete(sessionId)
      else next.set(sessionId, subagentId)
      return next
    })
  }, [])
  return (
    <SubagentsContext.Provider value={{ source, focused, focus }}>
      {children}
    </SubagentsContext.Provider>
  )
}

/** Outside a provider — a view rendered on its own, in a test — a window with no subagents. */
const none: SubagentsValue = {
  source: { forSession: () => undefined, subscribe: () => () => {}, send: () => {} },
  focused: new Map(),
  focus: () => {},
}

function useSubagentsContext(): SubagentsValue {
  return useContext(SubagentsContext) ?? none
}

/** A conversation's subagents as the source holds them now, followed while mounted. */
export function useSessionSubagents(sessionId: string): SessionSubagents | undefined {
  const { source } = useSubagentsContext()
  const subscribe = useCallback(
    (listener: () => void) => source.subscribe(listener),
    [source],
  )
  return useSyncExternalStore(subscribe, () => source.forSession(sessionId))
}

/** Which subagent a conversation's panel shows, and a way to change it. */
export function useFocusedSubagent(sessionId: string) {
  const { focused, focus } = useSubagentsContext()
  return [
    focused.get(sessionId) ?? null,
    useCallback(
      (subagentId: string | null) => focus(sessionId, subagentId),
      [focus, sessionId],
    ),
  ] as const
}

/**
 * The subagent a conversation's panel shows, for the panel's breadcrumb:
 * its name, and the way back to the list. Null while the list is shown.
 */
export function useSubagentTrail(sessionId: string) {
  const swarm = useSessionSubagents(sessionId)
  const [focused, setFocused] = useFocusedSubagent(sessionId)
  const subagent = swarm?.subagents.find((each) => each.id === focused)
  return subagent ? { label: subagent.name, onBack: () => setFocused(null) } : null
}

/** Says something to one of a conversation's subagents. */
export function useSendToSubagent(sessionId: string) {
  const { source } = useSubagentsContext()
  return useCallback(
    (subagentId: string, text: string) => source.send(sessionId, subagentId, text),
    [source, sessionId],
  )
}

/** Asks a conversation's panel to show one subagent, wherever the panel is. */
export function useFocusSubagent() {
  return useSubagentsContext().focus
}
