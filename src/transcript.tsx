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
          stack so a long thread still scrolls. Linux holds an unpainted
          reply-sized gap while the dots sit on the composer — same small
          pill as macOS, without sliding the user bubble. */}
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
            <LinuxThinking prompt={conversation.pending} />
          )
        ) : null}
      </div>
    </div>
  )
}

/**
 * WebKitGTK keeps the previous frame of a bubble that changes y. The
 * incoming reply's height is held as an invisible gap so the user turn
 * never slides; the dots are the same small pill macOS shows, sat on
 * the composer at the bottom of that gap.
 */
function LinuxThinking({ prompt }: { prompt: string }) {
  return (
    <div className="grid w-full">
      <div className="invisible col-start-1 row-start-1" aria-hidden="true">
        <ChatMessage tone="received" animateIn={false}>
          <ChatBubble>{draftReply(prompt)}</ChatBubble>
        </ChatMessage>
      </div>
      <div className="col-start-1 row-start-1 flex items-end">
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
