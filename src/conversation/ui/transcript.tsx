import * as React from "react"
import {
  ChatBubble,
  ChatMessage,
  ChatMessageActions,
  ChatMessageReceipt,
} from "@nessa-ui/react/chat-bubbles"
import { MessageStreamText } from "@nessa-ui/react/message"

import { type Conversation, type Receipt, type Turn } from "../model"
import { EmptyState } from "./empty-state"
import { Thinking } from "./thinking"

export function Transcript({
  conversation,
  ground,
  animateMount,
  streamText,
  emptyState,
  statusLabel,
}: {
  conversation: Conversation
  ground: "paper" | "ink"
  animateMount: boolean
  streamText: boolean
  emptyState: boolean
  statusLabel: string
}) {
  const logRef = React.useRef<HTMLDivElement>(null)
  const lastId = conversation.turns.at(-1)?.id
  const thinkingId = conversation.phase === "thinking" ? lastId : undefined
  const streamingId = conversation.phase === "streaming" ? lastId : undefined
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
            statusLabel={statusLabel}
          />
        ) : null}
        {conversation.turns.map((turn) => (
          <TurnRow
            key={turn.id}
            turn={turn}
            thinking={turn.id === thinkingId}
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
  thinking,
  streaming,
  animateMount,
}: {
  turn: Turn
  thinking: boolean
  streaming: boolean
  animateMount: boolean
}) {
  if (thinking) return <Thinking motion={animateMount} />

  return (
    <ChatMessage
      tone={turn.from === "user" ? "sent" : "received"}
      animateIn={animateMount}
    >
      <ChatBubble>
        {streaming ? <MessageStreamText text={turn.text} /> : turn.text}
      </ChatBubble>
      {turn.from === "user" ? (
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
