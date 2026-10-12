import { offersAuthenticationRecovery } from "../../provider-authentication/model/recovery"
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
import { ConversationQuestions } from "./conversation-questions"
import * as React from "react"
import {
  ChatBubble,
  ChatMessage,
  ChatMessageActions,
  ChatMessageReceipt,
} from "@nessa-ui/react/chat-bubbles"
import { MessageContentView } from "./message-content"
import { PublishedFiles } from "./published-files"
import { MessageMarkdown } from "@nessa-ui/react/message-markdown"
import { ProviderSignInCard } from "../../provider-authentication/ui/provider-sign-in"
import { TranscriptDivider } from "@nessa-ui/react/transcript-divider"

import {
  placePublished,
  type Conversation,
  type PublishedFile,
  type Receipt,
  type Turn,
} from "../model"
import { EmptyState } from "./empty-state"
import { Starting, Thinking } from "./thinking"
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
  onProviderSignIn,
  canSignInToProvider,
}: {
  conversation: Conversation
  ground: "paper" | "ink"
  animateMount: boolean
  streamText: boolean
  emptyState: boolean
  statusLabel: string
  gatewayAvailable: boolean
  onOpenPaste: (text: string) => void
  onProviderSignIn?: (provider: "claude" | "codex") => Promise<void>
  canSignInToProvider?: () => Promise<boolean>
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
  const linkedUserIds = new Set(
    rows.flatMap((row) => (row.promptId ? [row.promptId] : [])),
  )
  const waitingUsers = conversation.turns.filter(
    (turn) =>
      turn.from === "user" && turn.receipt === "queued" && !linkedUserIds.has(turn.id),
  )
  const localInputs = conversation.turns.flatMap((turn) =>
    turn.from === "user" && turn.actionId && turn.receipt !== "delivered"
      ? [{ observedInput: turn.observedInput }]
      : [],
  )
  const provider = conversation.remote?.runtime?.agent
  const sentTurns = conversation.turns.filter((turn) => turn.from === "user").length
  const published = publishedByRow(conversation, rows)

  return (
    <>
      <MessageScroller key={conversation.id} className="min-h-0 flex-1">
        <MessageScrollerViewport className="flex flex-col px-3 pb-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
          <MessageScrollerContent
            aria-label={`${conversation.title} transcript, ${sentTurns} sent`}
            className="mt-auto gap-5 select-text"
          >
            {conversation.turns.length === 0 &&
            // A file the agent published is something in the conversation,
            // even with no turn shown: it is listed, not an empty welcome.
            (conversation.remote?.artifacts ?? []).length === 0 &&
            conversation.phase === "idle" &&
            emptyState ? (
              <EmptyState
                seed={conversation.id}
                ground={ground}
                animateMount={animateMount}
                statusLabel={statusLabel}
              />
            ) : null}
            <PublishedFiles
              conversationId={conversation.serverConversationId}
              files={published.before}
            />
            {rows.map((row) => {
              const user = row.promptId ? users.get(row.promptId) : undefined
              return (
                <React.Fragment key={row.key}>
                  {user && (
                    <TurnRow
                      turn={user}
                      conversationId={conversation.serverConversationId}
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
                          conversationId={conversation.serverConversationId}
                          streaming={row.status === "running" && streamText}
                          animateMount={animateMount}
                          onOpenPaste={onOpenPaste}
                        />
                      )}
                      {"notice" in part && (
                        <p
                          role="status"
                          className="rounded-lg border border-border/70 bg-muted/50 px-3 py-2 text-xs [overflow-wrap:anywhere]"
                        >
                          {part.notice}
                        </p>
                      )}
                    </React.Fragment>
                  ))}
                  <PublishedFiles
                    conversationId={conversation.serverConversationId}
                    files={published.byRow.get(row.key) ?? []}
                  />
                  {(provider === "claude" || provider === "codex") &&
                  offersAuthenticationRecovery(
                    conversation.turns.find(
                      (turn) =>
                        turn.id === row.sourceTurnId &&
                        turn.from === "assistant" &&
                        turn.authenticationRequired,
                    )?.executionId,
                    conversation.remote?.latestInputId,
                    localInputs,
                  ) ? (
                    <React.Fragment key={`${row.key}:auth`}>
                      <ProviderSignInCard
                        provider={provider}
                        onSignIn={onProviderSignIn}
                        canSignIn={canSignInToProvider}
                      />
                      {row.status !== "failed" ? (
                        <TurnStatus status={row.status} />
                      ) : null}
                    </React.Fragment>
                  ) : (
                    <TurnStatus key={`${row.key}:status`} status={row.status} />
                  )}
                </React.Fragment>
              )
            })}
            <PublishedFiles
              conversationId={conversation.serverConversationId}
              files={published.after}
            />
            {waitingUsers.map((turn) => (
              <TurnRow
                key={`${turn.id}:waiting`}
                turn={turn}
                conversationId={conversation.serverConversationId}
                streaming={false}
                animateMount={animateMount}
                onOpenPaste={onOpenPaste}
              />
            ))}
            {conversation.phase === "thinking" &&
            (rows.length === 0 || rows.at(-1)?.status === "running") &&
            !conversation.readError &&
            !conversation.error ? (
              <Thinking motion={animateMount} />
            ) : null}
            {conversation.phase === "starting" &&
            !conversation.readError &&
            !conversation.error ? (
              <Starting />
            ) : null}
            <ConversationQuestions
              conversation={conversation}
              gatewayAvailable={gatewayAvailable}
            />
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
  conversationId,
  streaming,
  animateMount,
}: {
  turn: Turn
  /** The gateway's identity for the conversation, to read referenced images from. */
  conversationId: string | undefined
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
          <MessageContentView
            content={turn.content}
            conversationId={conversationId}
            onOpenPaste={onOpenPaste}
          />
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
 * The files the agent published, by row: under the row of the turn that was
 * running when each was published — the first row that names it — and the
 * rest before the first row or after the last, as `placePublished` rules.
 */
function publishedByRow(
  conversation: Conversation,
  rows: readonly { key: string; promptId?: string; sourceTurnId?: string }[],
): {
  byRow: Map<string, PublishedFile[]>
  before: PublishedFile[]
  after: PublishedFile[]
} {
  const files = conversation.remote?.artifacts ?? []
  const byRow = new Map<string, PublishedFile[]>()
  if (files.length === 0) return { byRow, before: [], after: [] }
  const executionOf = new Map(
    conversation.turns.flatMap((turn) =>
      turn.executionId ? [[turn.id, turn.executionId] as const] : [],
    ),
  )
  const rowOf = new Map<string, string>()
  for (const row of rows) {
    const execution =
      (row.promptId && executionOf.get(row.promptId)) ||
      (row.sourceTurnId && executionOf.get(row.sourceTurnId))
    if (execution && !rowOf.has(execution)) rowOf.set(execution, row.key)
  }
  const placed = placePublished(files, rowOf.keys())
  for (const [execution, published] of placed.byExecution) {
    const row = rowOf.get(execution)
    if (row !== undefined) byRow.set(row, published)
  }
  return { byRow, before: placed.before, after: placed.after }
}

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
