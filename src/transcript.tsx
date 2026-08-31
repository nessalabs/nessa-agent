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
import { type Conversation, type Receipt, type Turn } from "./conversation"
import { flushCompositor } from "./host-window"

/**
 * The turn list for the open conversation. Scrolls itself; does not know
 * how the turns got here. An empty assistant turn is thinking chrome —
 * the same row later holds the reply.
 */
export function Transcript({
  conversation,
  ground,
  animateMount,
  streamText,
  emptyState,
  flushOnTurn,
}: {
  conversation: Conversation
  ground: "paper" | "ink"
  animateMount: boolean
  streamText: boolean
  emptyState: boolean
  flushOnTurn: boolean
}) {
  const logRef = React.useRef<HTMLDivElement>(null)
  const streamingId =
    conversation.phase === "streaming" ? conversation.turns.at(-1)?.id : undefined
  const sentTurns = conversation.turns.filter((turn) => turn.from === "user").length
  const turnKey = `${conversation.id}:${conversation.phase}:${conversation.turns.length}`

  React.useEffect(() => {
    const log = logRef.current
    if (log) log.scrollTop = log.scrollHeight
  }, [conversation.turns, conversation.phase, conversation.id])

  React.useLayoutEffect(() => {
    if (!flushOnTurn) return
    void flushCompositor()
  }, [flushOnTurn, turnKey])

  return (
    <div
      role="log"
      ref={logRef}
      aria-label={`${conversation.title} transcript, ${sentTurns} sent`}
      className="flex min-h-0 flex-1 select-text flex-col overflow-y-auto px-3 pb-2 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
    >
      <div className="mt-auto flex flex-col gap-5">
        {conversation.turns.length === 0 &&
        conversation.phase === "idle" &&
        emptyState ? (
          <EmptyState
            seed={conversation.id}
            ground={ground}
            animateMount={animateMount}
          />
        ) : null}
        {conversation.turns.map((turn) => (
          <TurnRow
            key={turn.id}
            turn={turn}
            streaming={turn.id === streamingId && streamText}
            animateMount={animateMount}
          />
        ))}
      </div>
    </div>
  )
}

function TurnRow({
  turn,
  streaming,
  animateMount,
}: {
  turn: Turn
  streaming: boolean
  animateMount: boolean
}) {
  if (turn.from === "assistant" && turn.text === "") {
    return <Thinking motion={animateMount} />
  }

  return (
    <ChatMessage
      tone={turn.from === "user" ? "sent" : "received"}
      animateIn={animateMount}
    >
      <ChatBubble>
        {streaming ? <MessageStreamText text={turn.text} /> : turn.text}
      </ChatBubble>
      {turn.from === "user" && turn.receipt ? (
        <ChatMessageActions>
          <ChatMessageReceipt>{receiptLabel(turn.receipt)}</ChatMessageReceipt>
        </ChatMessageActions>
      ) : null}
    </ChatMessage>
  )
}

/**
 * The typing pill. Motion hosts use the design-system indicator (WAAPI pulse).
 * Layout hosts get the same pill without the translate animation — CSS cannot
 * cancel a WAAPI `translate`, and that is what ghosts on a transparent window.
 */
function Thinking({ motion }: { motion: boolean }) {
  if (motion) return <ChatTypingIndicator label="Nessa is typing" />
  return (
    <div
      role="status"
      aria-label="Nessa is typing"
      data-slot="chat-typing-indicator"
      className="flex items-center gap-1 self-start rounded-[1.125rem] bg-accent px-3.5 py-3"
    >
      {[0, 1, 2].map((dot) => (
        <span
          key={dot}
          data-slot="chat-typing-dot"
          className="size-2 rounded-full bg-muted-foreground opacity-35"
        />
      ))}
    </div>
  )
}

function receiptLabel(receipt: Receipt) {
  return receipt === "delivered" ? "Delivered" : "Sending"
}

function EmptyState({
  seed,
  ground,
  animateMount,
}: {
  seed: string
  ground: "paper" | "ink"
  animateMount: boolean
}) {
  return (
    <div className="flex flex-col items-center gap-2.5 py-6 text-center">
      <RandomAvatar
        seed={seed}
        hues={AGENT_HUES}
        name="Nessa"
        ground={ground}
        animateOnMount={animateMount}
        className="size-14 rounded-full"
      />
      <p className="nessa-text-3 m-0 text-muted-foreground">
        Nessa is listening. Press Enter to send.
      </p>
    </div>
  )
}
