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
  animate,
}: {
  conversation: Conversation
  ground: "paper" | "ink"
  /** False on WebKitGTK: mount springs leave compositor ghosts. */
  animate: boolean
}) {
  const logRef = React.useRef<HTMLDivElement>(null)
  const streamingId =
    conversation.phase === "streaming" ? conversation.turns.at(-1)?.id : undefined
  const stackKey = `${conversation.turns.length}:${conversation.phase}`

  React.useEffect(() => {
    const log = logRef.current
    if (log) log.scrollTop = log.scrollHeight
  }, [conversation.turns, conversation.phase, conversation.id])

  // WebKitGTK keeps the previous frame of a bubble that `mt-auto` slides up.
  // Hiding the stack before paint drops those tiles so Linux can stand on
  // the composer the same way macOS does, without a ghost under the reply.
  React.useLayoutEffect(() => {
    if (animate) return
    const node = logRef.current
    if (!node) return
    node.style.visibility = "hidden"
    void node.offsetHeight
    node.style.visibility = ""
  }, [animate, stackKey])

  return (
    <div
      role="log"
      aria-label={`${conversation.title} transcript`}
      className="flex min-h-0 flex-1 select-text flex-col px-3 pb-2"
    >
      {/* Sized to the turns, stood on the composer. Overflow lives on this
          stack so a long thread still scrolls. The stack itself is not a
          compositing layer: that painted a white slab behind the bubbles.
          Keyed per turn so WebKitGTK paints the whole group at the new
          bottom instead of sliding a live layer. */}
      <div
        key={stackKey}
        ref={logRef}
        className="mt-auto flex min-h-0 flex-col gap-5 overflow-y-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
      >
        {conversation.turns.length === 0 && conversation.phase === "idle" && animate ? (
          <EmptyState seed={conversation.id} ground={ground} />
        ) : null}
        {conversation.turns.map((turn) => (
          <ChatMessage
            key={turn.id}
            tone={turn.from === "user" ? "sent" : "received"}
            animateIn={animate}
          >
            <ChatBubble>
              {turn.id === streamingId && animate ? (
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
          animate ? (
            <ChatTypingIndicator label="Nessa is typing" />
          ) : (
            <div
              role="status"
              aria-label="Nessa is typing"
              className="flex items-center gap-1 self-start rounded-[1.125rem] bg-accent px-3.5 py-3"
            >
              {[0, 1, 2].map((dot) => (
                <span
                  key={dot}
                  className="size-2 rounded-full bg-muted-foreground opacity-35"
                />
              ))}
            </div>
          )
        ) : null}
      </div>
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
