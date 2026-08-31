import * as React from "react"
import {
  ChatBubble,
  ChatMessage,
  ChatMessageActions,
  ChatMessageReceipt,
} from "@nessa-ui/react/chat-bubbles"
import { MessageStreamText } from "@nessa-ui/react/message"

import { useFlushOnTurn } from "../adapters/compositor-flush"
import { type Conversation, type Receipt, type Turn } from "../model"
import { EmptyState } from "./empty-state"
import { Thinking } from "./thinking"

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

  useFlushOnTurn(flushOnTurn, turnKey)

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

function receiptLabel(receipt: Receipt) {
  return receipt === "delivered" ? "Delivered" : "Sending"
}
