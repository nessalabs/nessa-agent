import { useEffect } from "react"

import { STREAMING_MS, THINKING_MS, type Phase } from "./conversation"
import { advanceReply, useAppDispatch, useAppSelector } from "./store"

/**
 * The stand-in runtime's clocks. One timer per conversation so a reply
 * arriving in a background tab does not restart another conversation's clock.
 */
export function ConversationClocks() {
  const conversations = useAppSelector((state) => state.conversation.conversations)

  return (
    <>
      {conversations.map((item) => (
        <ReplyTimer key={item.id} conversationId={item.id} phase={item.phase} />
      ))}
    </>
  )
}

function ReplyTimer({ conversationId, phase }: { conversationId: string; phase: Phase }) {
  const dispatch = useAppDispatch()

  useEffect(() => {
    if (phase === "idle") return
    const delay = phase === "thinking" ? THINKING_MS : STREAMING_MS
    const timer = window.setTimeout(() => {
      dispatch(advanceReply({ conversationId }))
    }, delay)
    return () => window.clearTimeout(timer)
  }, [conversationId, dispatch, phase])

  return null
}
