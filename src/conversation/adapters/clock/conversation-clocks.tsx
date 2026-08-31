import { useEffect } from "react"

import { STREAMING_MS, THINKING_MS } from "../../application/delays"
import type { Phase } from "../../model"
import { advanceReply } from "../store/slice"
import { useConversationDispatch, useConversationSelector } from "../store/hooks"

/**
 * Stand-in for the server driving `idle → thinking → streaming`. One timer
 * per conversation so a background tab does not restart another clock.
 */
export function ConversationClocks() {
  const conversations = useConversationSelector(
    (state) => state.conversation.conversations,
  )

  return (
    <>
      {conversations.map((item) => (
        <ReplyTimer key={item.id} conversationId={item.id} phase={item.phase} />
      ))}
    </>
  )
}

function ReplyTimer({ conversationId, phase }: { conversationId: string; phase: Phase }) {
  const dispatch = useConversationDispatch()

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
