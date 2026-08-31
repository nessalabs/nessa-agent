import * as React from "react"

import {
  advance,
  closeInStrip,
  conversation,
  send,
  stop,
  STREAMING_MS,
  THINKING_MS,
  type Conversation,
  type Phase,
} from "./conversation"

/**
 * The conversation strip: which thread is open, each thread's turns and
 * draft, and the stand-in clock that walks `idle → thinking → streaming`.
 *
 * One timer per conversation, so a reply arriving in a background tab does
 * not restart the clock on another.
 */
export function useConversation() {
  const nextId = React.useRef(1)
  const [conversations, setConversations] = React.useState<Conversation[]>(() => [
    conversation("c0"),
  ])
  const [activeId, setActiveId] = React.useState("c0")
  const activeIdRef = React.useRef(activeId)
  activeIdRef.current = activeId

  function takeId() {
    return nextId.current++
  }

  function update(id: string, change: (current: Conversation) => Conversation) {
    setConversations((items) =>
      items.map((item) => (item.id === id ? change(item) : item)),
    )
  }

  const active =
    conversations.find((item) => item.id === activeId) ?? conversations[0]!

  function submit() {
    if (active.draft.trim() === "" || active.phase !== "idle") return
    const turnId = `t${takeId()}`
    update(active.id, (current) => send(current, current.draft, turnId))
  }

  function openConversation() {
    const id = `c${takeId()}`
    setConversations((items) => [...items, conversation(id)])
    setActiveId(id)
  }

  function closeConversation(id: string) {
    setConversations((items) => {
      const next = closeInStrip(items, id, activeIdRef.current, () => `c${takeId()}`)
      activeIdRef.current = next.activeId
      setActiveId(next.activeId)
      return next.items
    })
  }

  function setDraft(draft: string) {
    update(active.id, (current) => ({ ...current, draft }))
  }

  function stopGenerating() {
    update(active.id, stop)
  }

  const clocks = (
    <>
      {conversations.map((item) => (
        <ReplyTimer
          key={item.id}
          phase={item.phase}
          onAdvance={() => update(item.id, (current) => advance(current, `t${takeId()}`))}
        />
      ))}
    </>
  )

  return {
    conversations,
    active,
    setActiveId,
    submit,
    openConversation,
    closeConversation,
    setDraft,
    stopGenerating,
    clocks,
  }
}

/**
 * The stand-in runtime's clock for one conversation. Owning it per
 * conversation is what keeps background threads streaming on their own
 * schedule; a single shared effect would reset every timer whenever any one
 * of them advanced.
 */
function ReplyTimer({ phase, onAdvance }: { phase: Phase; onAdvance: () => void }) {
  const latest = React.useRef(onAdvance)
  React.useEffect(() => {
    latest.current = onAdvance
  })

  React.useEffect(() => {
    if (phase === "idle") return
    const delay = phase === "thinking" ? THINKING_MS : STREAMING_MS
    const timer = window.setTimeout(() => latest.current(), delay)
    return () => window.clearTimeout(timer)
  }, [phase])

  return null
}
