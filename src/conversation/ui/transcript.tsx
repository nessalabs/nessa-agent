import { agentTranscript } from "../adapters/agent-stream/transcript"
import { agentTurnView } from "./agent-transcript-view"
import { WorkActivity, WorkDetails } from "./work-activity"
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
import { TranscriptDivider } from "@nessa-ui/react/transcript-divider"

import { type Conversation, type Receipt, type Turn } from "../model"
import { EmptyState } from "./empty-state"
import { Thinking } from "./thinking"
import { selectedWork } from "./work-selection"

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
  // Scoped to the conversation, so a key that happens to recur in the next
  // tab does not open that tab's sheet.
  const [workFor, setWorkFor] = React.useState<{
    conversationId: string
    key: string
  } | null>(null)
  const sheetId = React.useId()
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
  const openWork = selectedWork(
    segments,
    workFor?.conversationId === conversation.id ? workFor.key : null,
  )
  React.useEffect(() => {
    if (workFor !== null && !openWork) setWorkFor(null)
  }, [openWork, workFor])
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
                      {part.work && (
                        <WorkActivity
                          work={part.work}
                          running={part.running ?? false}
                          seed={conversation.id}
                          expanded={
                            workFor?.conversationId === conversation.id &&
                            workFor.key === part.key
                          }
                          sheetId={sheetId}
                          onOpen={() =>
                            setWorkFor({ conversationId: conversation.id, key: part.key })
                          }
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
                  <TurnStatus key={`${row.key}:status`} status={row.status} />
                </React.Fragment>
              )
            })}
            {conversation.phase === "thinking" &&
            (rows.length === 0 || rows.at(-1)?.status === "running") &&
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
      {openWork?.work && (
        <WorkDetails
          work={openWork.work}
          running={openWork.running ?? false}
          sheetId={sheetId}
          onClose={() => setWorkFor(null)}
        />
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
      {turn.from === "user" ? (
        <ChatMessageActions>
          <ChatMessageReceipt>{receiptLabel(turn.receipt)}</ChatMessageReceipt>
        </ChatMessageActions>
      ) : null}
    </ChatMessage>
  )
})

/**
 * How a turn ended, when it did not simply finish. It belongs to the turn, not
 * to a bubble — a turn cancelled before it said anything has no bubble to hang
 * it on — and it is a mark on the transcript rather than a line the agent
 * said, so it is drawn as the same rule the compaction divider draws.
 */
function TurnStatus({ status }: { status: string }) {
  if (["running", "completed"].includes(status)) return null
  return (
    <TranscriptDivider role="status" className="[overflow-wrap:anywhere]">
      {status === "cancelled" ? "Cancelled" : status}
    </TranscriptDivider>
  )
}

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
