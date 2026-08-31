import * as React from "react"
import {
  ChatBubble,
  ChatMessage,
  ChatMessageActions,
  ChatMessageReceipt,
  ChatTypingIndicator,
} from "@nessa-ui/react/chat-bubbles"
import { MessageStreamText } from "@nessa-ui/react/message"
import { RandomAvatar } from "@nessa-ui/react/random-avatar"

import { AGENT_HUES } from "./agent-identity"
import type { Conversation } from "./conversation"

/**
 * The turn list for the open conversation. Scrolls itself; does not know
 * how the turns got here.
 */
export function Transcript({
  conversation,
  ground,
}: {
  conversation: Conversation
  ground: "paper" | "ink"
}) {
  const logRef = React.useRef<HTMLDivElement>(null)
  const streamingId =
    conversation.phase === "streaming" ? conversation.turns.at(-1)?.id : undefined

  React.useEffect(() => {
    const log = logRef.current
    if (log) log.scrollTop = log.scrollHeight
  }, [conversation.turns, conversation.phase, conversation.id])

  return (
    <div
      ref={logRef}
      role="log"
      aria-label={`${conversation.title} transcript`}
      className="flex min-h-0 flex-1 select-text flex-col gap-5 overflow-y-auto px-3 pb-4 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
    >
      {/* Bottom-anchors a short transcript without justify-end, which
          would trap overflowing messages above an unscrollable top. */}
      <div aria-hidden="true" className="mt-auto shrink-0" />
      {conversation.turns.length === 0 && conversation.phase === "idle" ? (
        <EmptyState seed={conversation.id} ground={ground} />
      ) : null}
      {conversation.turns.map((turn) => (
        <ChatMessage key={turn.id} tone={turn.from === "user" ? "sent" : "received"}>
          <ChatBubble>
            {turn.id === streamingId ? (
              <MessageStreamText text={turn.text} />
            ) : (
              turn.text
            )}
          </ChatBubble>
          {turn.from === "user" ? (
            <ChatMessageActions>
              <ChatMessageReceipt>Delivered</ChatMessageReceipt>
            </ChatMessageActions>
          ) : null}
        </ChatMessage>
      ))}
      {conversation.phase === "thinking" ? (
        <ChatTypingIndicator label="Nessa is typing" />
      ) : null}
    </div>
  )
}

function EmptyState({ seed, ground }: { seed: string; ground: "paper" | "ink" }) {
  return (
    <div className="flex flex-col items-center gap-2.5 py-6 text-center">
      <RandomAvatar
        seed={seed}
        hues={AGENT_HUES}
        name="Nessa"
        ground={ground}
        animateOnMount
        className="size-14 rounded-full"
      />
      <p className="nessa-text-3 m-0 text-muted-foreground">
        Nessa is listening. Press Enter to send.
      </p>
    </div>
  )
}
