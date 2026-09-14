import * as React from "react"
import {
  ChatBubble,
  ChatMessage,
  ChatMessageActions,
  ChatMessageReceipt,
} from "@nessa-ui/react/chat-bubbles"
import { MessageContentView } from "./message-content"
import { MessageMarkdown } from "@nessa-ui/react/message-markdown"
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
  onOpenPaste,
}: {
  conversation: Conversation
  ground: "paper" | "ink"
  animateMount: boolean
  streamText: boolean
  emptyState: boolean
  statusLabel: string
  onOpenPaste: (text: string) => void
}) {
  const logRef = React.useRef<HTMLDivElement>(null)
  const lastId = conversation.turns.at(-1)?.id
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
            streaming={turn.id === streamingId && streamText}
            animateMount={animateMount}
            onOpenPaste={onOpenPaste}
          />
        ))}
        {conversation.phase === "thinking" ? <Thinking motion={animateMount} /> : null}
      </div>
    </div>
  )
}

const TurnRow = React.memo(function TurnRow({
  onOpenPaste,
  turn,
  streaming,
  animateMount,
}: {
  turn: Turn
  onOpenPaste: (text: string) => void
  streaming: boolean
  animateMount: boolean
}) {
  return (
    <ChatMessage
      tone={turn.from === "user" ? "sent" : "received"}
      animateIn={animateMount}
    >
      <ChatBubble>
        {turn.from === "user" ? (
          <MessageContentView content={turn.content} onOpenPaste={onOpenPaste} />
        ) : streaming ? (
          <MessageStreamText text={turn.text} />
        ) : (
          <MessageMarkdown>{turn.text}</MessageMarkdown>
        )}
      </ChatBubble>
      {turn.from === "user" ? (
        <ChatMessageActions>
          <ChatMessageReceipt>{receiptLabel(turn.receipt)}</ChatMessageReceipt>
        </ChatMessageActions>
      ) : null}
    </ChatMessage>
  )
})

function receiptLabel(receipt: Receipt) {
  return receipt === "delivered" ? "Delivered" : "Sending"
}
