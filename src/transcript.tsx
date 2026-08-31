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
import { draftReply, type Conversation } from "./conversation"

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
  const sentTurns = conversation.turns.filter((turn) => turn.from === "user").length

  React.useEffect(() => {
    const log = logRef.current
    if (log) log.scrollTop = log.scrollHeight
  }, [conversation.turns, conversation.phase, conversation.id])

  return (
    <div
      role="log"
      ref={logRef}
      aria-label={`${conversation.title} transcript, ${sentTurns} sent`}
      data-turn-count={conversation.turns.length}
      data-sent-turns={sentTurns}
      className="flex min-h-0 flex-1 select-text flex-col px-3 pb-2"
    >
      {/* Sized to the turns, stood on the composer. Overflow lives on this
          stack so a long thread still scrolls. Linux reserves the reply's
          height while thinking so the user bubble never slides — a slide
          leaves the previous tile on WebKitGTK. */}
      <div className="mt-auto flex min-h-0 flex-col gap-5 overflow-y-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
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
            <ReservedReply prompt={conversation.pending} />
          )
        ) : null}
      </div>
    </div>
  )
}

/**
 * WebKitGTK keeps the previous frame of a bubble that changes y. Holding
 * the incoming reply's height under the dots means the user turn is born
 * where it will stay, so one send cannot leave a second "hey".
 */
function ReservedReply({ prompt }: { prompt: string }) {
  return (
    <ChatMessage tone="received" animateIn={false}>
      <ChatBubble>
        <span className="grid">
          <span className="invisible col-start-1 row-start-1">{draftReply(prompt)}</span>
          <span
            role="status"
            aria-label="Nessa is typing"
            className="col-start-1 row-start-1 flex items-center gap-1 self-center"
          >
            {[0, 1, 2].map((dot) => (
              <span
                key={dot}
                className="size-2 rounded-full bg-muted-foreground opacity-35"
              />
            ))}
          </span>
        </span>
      </ChatBubble>
    </ChatMessage>
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
