import { memo, useContext, useSyncExternalStore, type ReactNode } from "react"
import { ReactReduxContext } from "react-redux"
import type { SessionSummary } from "../../model/workspace-index"
import { openSession } from "../../adapters/store/commands"
import type { DesktopStore } from "../../../store"
import { AgentTile } from "../chrome/agent-tile"
import "./chat-pill.css"

/**
 * Another chat, named where a transcript speaks of it: its agent and its
 * title, as they are now (a rename shows at once). Pressed, it opens that
 * chat as any opening does — its tab brought forward, or a tab of its own
 * (`openSession`). A chat the window no longer has is a quiet pill that says
 * so and goes nowhere. One pill for every place: a thread's report line, an
 * id an agent wrote, a tool that named a chat.
 */
export const ChatPill = memo(function ChatPill({
  sessionId,
  still = false,
}: {
  sessionId: string
  /**
   * Drawn, not pressed: inside a control of its own — a tool step's row —
   * the pill names the chat with the same look, and the row keeps the press.
   */
  still?: boolean
}) {
  const store = useOptionalStore()
  const chats = useChats()
  // An id comes from text anyone wrote: `constructor` is not a chat.
  const session = Object.hasOwn(chats, sessionId)
    ? (chats[sessionId] as SessionSummary)
    : undefined
  if (!session)
    return (
      <span className="workspace-chat-pill" data-gone title="This chat is no longer here">
        <span className="workspace-chat-pill-title">A chat that’s gone</span>
      </span>
    )
  if (still)
    return (
      <span className="workspace-chat-pill" data-chat-pill={sessionId} data-still>
        <AgentTile model={session.model} size={14} />
        <span className="workspace-chat-pill-title">{session.title}</span>
      </span>
    )
  return (
    <button
      type="button"
      className="workspace-chat-pill"
      data-chat-pill={sessionId}
      title={`Open ${session.title}`}
      onClick={() => store?.dispatch(openSession({ sessionId }))}
    >
      <AgentTile model={session.model} size={14} />
      <span className="workspace-chat-pill-title">{session.title}</span>
    </button>
  )
})

const none: Readonly<Record<string, SessionSummary>> = {}
const unsubscribe = () => () => {}

/** The window's store, where there is one: a transcript drawn alone has none. */
function useOptionalStore(): DesktopStore | null {
  return (useContext(ReactReduxContext)?.store as DesktopStore | undefined) ?? null
}

/** The chats the window has, by id; none where there is no window. */
function useChats(): Readonly<Record<string, SessionSummary>> {
  const store = useOptionalStore()
  return useSyncExternalStore(store ? store.subscribe : unsubscribe, () =>
    store ? store.getState().workspace.sessions : none,
  )
}

/** Whether the window has a chat with this id: only those are drawn as pills. */
export function useKnownChat(id: string): boolean {
  return Object.hasOwn(useChats(), id)
}

const chatId = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi

/**
 * A line of the window's own — a tool step's title or what it acted on —
 * with each chat id the window knows drawn as that chat's pill (`still`
 * inside a control of its own). An id it does not know stays as text.
 */
export function WithChats({ text, still = false }: { text: string; still?: boolean }) {
  const sessions = useChats()
  const parts: ReactNode[] = []
  let at = 0
  for (const match of text.matchAll(chatId)) {
    const id = match[0]
    if (!Object.hasOwn(sessions, id)) continue
    const index = match.index ?? 0
    if (index > at) parts.push(text.slice(at, index))
    parts.push(<ChatPill key={index} sessionId={id} still={still} />)
    at = index + id.length
  }
  if (parts.length === 0) return <>{text}</>
  if (at < text.length) parts.push(text.slice(at))
  return <>{parts}</>
}
