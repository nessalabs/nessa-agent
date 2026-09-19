import { agentTranscript } from "../adapters/agent-stream/transcript"
import { agentTurnView } from "./agent-transcript-view"
import {
  ToolActivity,
  ToolDetails,
  ThoughtActivity,
  ThoughtDetails,
} from "./tool-activity"
import {
  MessageScroller,
  MessageScrollerViewport,
  MessageScrollerContent,
  MessageScrollerButton,
} from "@nessa-ui/react/message-scroller"
import { ConversationControls } from "./conversation-controls"
import * as React from "react"
import {
  ChatBubble,
  ChatMessage,
  ChatMessageActions,
  ChatMessageReceipt,
} from "@nessa-ui/react/chat-bubbles"
import { MessageContentView } from "./message-content"
import { MessageMarkdown } from "@nessa-ui/react/message-markdown"

import { type Conversation, type Receipt, type Turn } from "../model"
import { EmptyState } from "./empty-state"
import { Thinking } from "./thinking"
import { selectedToolActivity } from "./tool-selection"

export function Transcript({
  conversation,
  ground,
  animateMount,
  streamText,
  emptyState,
  statusLabel,
  gatewayAvailable,
  onOpenPaste,
}: {
  conversation: Conversation
  ground: "paper" | "ink"
  animateMount: boolean
  streamText: boolean
  emptyState: boolean
  statusLabel: string
  gatewayAvailable: boolean
  onOpenPaste: (text: string) => void
}) {
  const [thoughtFor, setThoughtFor] = React.useState<string | null>(null)
  const [toolsFor, setToolsFor] = React.useState<string | null>(null)
  const normalized = React.useMemo(
    () =>
      agentTranscript(
        conversation.id,
        conversation.turns,
        conversation.remote?.tools ?? [],
      ),
    [conversation.id, conversation.turns, conversation.remote?.tools],
  )
  const rows = React.useMemo(
    () => normalized.turns.map((turn) => agentTurnView(turn, normalized)),
    [normalized],
  )
  const segments = rows.flatMap((row) => row.content)
  const thoughtTurn = segments.find((part) => part.key === thoughtFor)
  const selectedToolPart = selectedToolActivity(segments, toolsFor)
  const selectedTools = selectedToolPart?.tools ?? []
  React.useEffect(() => {
    if (toolsFor !== null && !selectedToolPart) setToolsFor(null)
  }, [selectedToolPart, toolsFor])
  const users = new Map(
    conversation.turns
      .filter((turn) => turn.from === "user")
      .map((turn) => [turn.id, turn]),
  )
  const sentTurns = conversation.turns.filter((turn) => turn.from === "user").length

  return (
    <>
      <MessageScroller key={conversation.id} className="min-h-0 flex-1">
        <MessageScrollerViewport className="flex flex-col px-3 pb-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          <MessageScrollerContent
            aria-label={`${conversation.title} transcript, ${sentTurns} sent`}
            className="mt-auto gap-5 select-text"
          >
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
            {rows.map((row) => {
              const user = row.promptId ? users.get(row.promptId) : undefined
              return (
                <React.Fragment key={row.key}>
                  {user && (
                    <TurnRow
                      turn={user}
                      streaming={false}
                      animateMount={animateMount}
                      onOpenPaste={onOpenPaste}
                    />
                  )}
                  {row.content.map((part) => (
                    <React.Fragment key={part.key}>
                      {part.thought && (
                        <ThoughtActivity
                          onOpen={() => setThoughtFor(part.key)}
                          running={row.status === "running"}
                        />
                      )}
                      {part.tools && (
                        <ToolActivity
                          tools={part.tools}
                          onOpen={() => setToolsFor(part.key)}
                        />
                      )}
                      {part.text && (
                        <TurnRow
                          turn={{
                            id: part.key,
                            from: "assistant",
                            parts: [],
                            text: part.text,
                            status: row.status,
                          }}
                          streaming={row.status === "running" && streamText}
                          animateMount={animateMount}
                          onOpenPaste={onOpenPaste}
                        />
                      )}
                    </React.Fragment>
                  ))}
                </React.Fragment>
              )
            })}
            {conversation.phase === "thinking" &&
            !conversation.readError &&
            !conversation.error ? (
              <Thinking motion={animateMount} />
            ) : null}
            <ConversationControls
              conversation={conversation}
              gatewayAvailable={gatewayAvailable}
            />
          </MessageScrollerContent>
        </MessageScrollerViewport>
        <MessageScrollerButton />
      </MessageScroller>
      {thoughtTurn && (
        <ThoughtDetails
          thought={thoughtTurn.thought ?? ""}
          onClose={() => setThoughtFor(null)}
        />
      )}
      {selectedToolPart && (
        <ToolDetails tools={selectedTools} onClose={() => setToolsFor(null)} />
      )}
    </>
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
        ) : (
          <MessageMarkdown streaming={streaming}>{turn.text}</MessageMarkdown>
        )}
      </ChatBubble>
      {turn.from === "assistant" &&
      turn.status &&
      !["running", "completed"].includes(turn.status) ? (
        <p role="status" className="text-xs">
          {turn.status === "cancelled" ? "Cancelled" : turn.status}
        </p>
      ) : null}
      {turn.from === "user" ? (
        <ChatMessageActions>
          <ChatMessageReceipt>{receiptLabel(turn.receipt)}</ChatMessageReceipt>
        </ChatMessageActions>
      ) : null}
    </ChatMessage>
  )
})

function receiptLabel(receipt: Receipt) {
  return {
    delivered: "Seen",
    accepted: "Sent",
    queued: "Queued",
    sending: "Sending",
    unknown: "Delivery unknown",
    failed: "Not sent",
  }[receipt]
}
