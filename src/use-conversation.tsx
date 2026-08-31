import * as React from "react"

import { STREAMING_MS, THINKING_MS, type Phase } from "./conversation"
import {
  advanceReply,
  closeConversation,
  openConversation,
  sendDraft,
  setActiveId,
  setDraft,
  stopGenerating,
  useAppDispatch,
  useAppSelector,
} from "./store"

/**
 * The conversation strip on screen: selectors, the actions the chrome
 * dispatches, and the stand-in clock that walks `idle → thinking → streaming`.
 *
 * One timer per conversation, so a reply arriving in a background tab does
 * not restart the clock on another. The clock dispatches; it does not own
 * the strip.
 */
export function useConversation() {
  const dispatch = useAppDispatch()
  const conversations = useAppSelector((state) => state.conversation.conversations)
  const activeId = useAppSelector((state) => state.conversation.activeId)
  const active = conversations.find((item) => item.id === activeId) ?? conversations[0]!

  const clocks = (
    <>
      {conversations.map((item) => (
        <ReplyTimer
          key={item.id}
          phase={item.phase}
          onAdvance={() => dispatch(advanceReply({ id: item.id }))}
        />
      ))}
    </>
  )

  return {
    conversations,
    active,
    setActiveId: (id: string) => dispatch(setActiveId(id)),
    submit: () => dispatch(sendDraft()),
    openConversation: () => {
      dispatch(openConversation())
    },
    closeConversation: (id: string) => dispatch(closeConversation(id)),
    setDraft: (draft: string) => dispatch(setDraft({ draft })),
    stopGenerating: () => dispatch(stopGenerating()),
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
  }, [onAdvance])

  React.useEffect(() => {
    if (phase === "idle") return
    const delay = phase === "thinking" ? THINKING_MS : STREAMING_MS
    const timer = window.setTimeout(() => latest.current(), delay)
    return () => window.clearTimeout(timer)
  }, [phase])

  return null
}
