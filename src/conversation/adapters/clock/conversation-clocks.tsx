import { useEffect } from "react"

import { STREAMING_MS, THINKING_MS } from "../../application/delays"
import type { Phase } from "../../model"
import { advanceReply } from "../store/slice"
import { useConversationDispatch, useConversationSelector } from "../store/hooks"

export function ConversationClocks() {
  const busy = useConversationSelector((state) =>
    state.conversation.conversations.filter((item) => item.phase !== "idle"),
  )

  return (
    <>
      {busy.map((item) => (
        <ReplyTimer key={item.id} conversationId={item.id} phase={item.phase} />
      ))}
    </>
  )
}

function ReplyTimer({
  conversationId,
  phase,
}: {
  conversationId: string
  phase: Exclude<Phase, "idle">
}) {
  const dispatch = useConversationDispatch()

  useEffect(() => {
    const delay = phase === "thinking" ? THINKING_MS : STREAMING_MS
    const timer = window.setTimeout(() => {
      dispatch(advanceReply({ conversationId }))
    }, delay)
    return () => window.clearTimeout(timer)
  }, [conversationId, dispatch, phase])

  return null
}
