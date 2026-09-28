import { memo, type RefObject } from "react"
import { DesktopIcon } from "../../../ui/icons"
import { tooltip } from "../../../ui/tooltip"
import { useWorkspaceSelector } from "../../../workspace"
import { useNow } from "../../../workspace/adapters/dom/clock"
import { labelOf } from "../../../workspace/adapters/dom/shortcuts"
import type { WorkspaceFailureReason } from "../../../workspace/model/failure"
import {
  agentName,
  agentOf,
  modelName,
  type SessionSummary,
} from "../../../workspace/model/workspace-index"
import { sessionTime } from "../../../workspace/model/time-labels"
import type { Approval } from "../../../workspace/model/transcript"
import { AgentTile } from "../../../workspace/ui/chrome/agent-tile"
import { StatusGlyph } from "../../../workspace/ui/chrome/status-glyph"
import { failureCopy } from "../../../workspace/ui/failure-copy"
import { LiveRow } from "../../../workspace/ui/transcript/live-row"
import {
  ApprovalActions,
  ApprovalCommand,
} from "../../../workspace/ui/transcript/approval-request"
import { ToolSteps } from "../../../workspace/ui/transcript/tool-steps"
import "../../../workspace/ui/transcript/transcript.css"
import { selectChannelName, selectSummary } from "../adapters/workspace-bridge"
import { peekOf } from "../model/peek"
import { answeredLabels, type Answer, type Settling } from "../model/request"
import { overviewKeys } from "./overview-keys"
import { ReplyPill } from "./reply-pill"
import { useConversation } from "./use-request"

/**
 * A look into a session without leaving the overview: what its agent is
 * doing now, what it has done this turn, the last thing it said — and, when
 * it waits on the person, the request in full with its answers. Drawn with
 * the conversation's own pieces (the live row, the steps), so a session reads
 * the same here as in its pane. A reply pill at its foot lets the person
 * answer the agent from here.
 */
export const SessionPeek = memo(function SessionPeek({
  sessionId,
  settling,
  failure,
  onOpen,
  onAnswer,
  onLeaveReply,
  replyRef,
}: {
  sessionId: string
  settling: Settling | undefined
  failure:
    { readonly approvalId: string; readonly reason: WorkspaceFailureReason } | undefined
  onOpen: (sessionId: string) => void
  onAnswer: (summary: SessionSummary, approval: Approval, choice: Answer) => void
  /** Escape in the reply pill: the keyboard goes back to the list. */
  onLeaveReply: () => void
  replyRef?: RefObject<HTMLTextAreaElement | null>
}) {
  const live = useWorkspaceSelector((state) => selectSummary(state, sessionId))
  const summary = settling?.summary ?? live
  const { transcript, failure: unreadable } = useConversation(sessionId, live?.revision)
  const channel = useWorkspaceSelector((state) =>
    summary ? selectChannelName(state, summary.channelId) : undefined,
  )
  const now = useNow(30_000)
  if (!summary) return null
  const peek = transcript ? peekOf(transcript) : null
  const waiting = settling !== undefined || summary.status === "needs-you"
  const approval = settling?.approval ?? (waiting ? (transcript?.approval ?? null) : null)
  const answerable = approval !== null && !settling
  const refused =
    failure && approval && failure.approvalId === approval.id ? failure.reason : null
  const agent = agentName(agentOf(summary.model))
  const reply = (
    <ReplyPill
      sessionId={sessionId}
      agent={agent}
      fieldRef={replyRef}
      onLeave={onLeaveReply}
      note={
        approval && !settling
          ? "Sending a reply sets this request aside without running it."
          : undefined
      }
    />
  )

  return (
    <article className="agents-peek" aria-label={`${summary.title}, at a glance`}>
      <header className="agents-peek-head">
        <AgentTile model={summary.model} size={32} />
        <div className="agents-peek-titles">
          <h2>{summary.title}</h2>
          <p className="agents-truncate">
            {agent} · {modelName(summary.model)}
            {channel ? ` · #${channel}` : ""}
          </p>
        </div>
        <button
          type="button"
          className="workspace-button agents-peek-open"
          {...tooltip("Open Session", { shortcut: labelOf(overviewKeys, "open") })}
          onClick={() => onOpen(sessionId)}
        >
          Open
          <DesktopIcon name="forward" />
        </button>
      </header>

      <div className="agents-peek-now">
        {summary.status === "running" && peek?.activity ? (
          <LiveRow activity={peek.activity} />
        ) : (
          <p className="agents-peek-state" data-status={summary.status}>
            <StatusGlyph status={waiting ? "needs-you" : summary.status} idle />
            <span>
              {waiting
                ? "Waiting for you"
                : summary.status === "running"
                  ? "Writing a reply"
                  : "Finished"}
            </span>
            <time dateTime={new Date(summary.updatedAt).toISOString()}>
              {sessionTime(summary.updatedAt, now)}
            </time>
          </p>
        )}
      </div>

      {approval ? (
        <section className="agents-peek-ask" aria-label="Request">
          <p className="agents-peek-reason">{approval.reason}</p>
          <ApprovalCommand command={approval.command} />
          {refused ? (
            <p className="agents-peek-failure" role="status">
              {failureCopy(refused)}
            </p>
          ) : null}
          {settling ? (
            <p className="agents-peek-answered" data-answer={settling.answer}>
              <DesktopIcon name={settling.answer === "deny" ? "close" : "check"} />
              {answeredLabels[settling.answer]}
            </p>
          ) : (
            <ApprovalActions
              disabled={!answerable}
              tips={{
                deny: tooltip("Don’t run it", {
                  shortcut: labelOf(overviewKeys, "deny"),
                }),
                always: tooltip("Allow it now, and whenever it’s asked again", {
                  shortcut: labelOf(overviewKeys, "always"),
                }),
                once: tooltip("Run it once", {
                  shortcut: labelOf(overviewKeys, "allow"),
                }),
              }}
              onAnswer={(choice) =>
                answerable &&
                onAnswer(summary, approval, choice === "once" ? "allow" : choice)
              }
            />
          )}
        </section>
      ) : null}

      {peek && peek.steps.length > 0 ? (
        <section
          className="agents-peek-section"
          aria-labelledby={`peek-steps-${sessionId}`}
        >
          <h3 id={`peek-steps-${sessionId}`}>
            This turn
            {peek.earlier > 0 ? (
              <span>
                {peek.earlier} earlier {peek.earlier === 1 ? "step" : "steps"}
              </span>
            ) : null}
          </h3>
          <ToolSteps steps={peek.steps} />
        </section>
      ) : null}

      {peek?.said ? (
        <section
          className="agents-peek-section"
          aria-labelledby={`peek-said-${sessionId}`}
        >
          <h3 id={`peek-said-${sessionId}`}>
            {peek.said.by === "you" ? "You said" : "Last said"}
            <span>{sessionTime(peek.said.at, now)}</span>
          </h3>
          <p className="agents-peek-said">{peek.said.text}</p>
        </section>
      ) : null}

      {!transcript && unreadable ? (
        <p className="agents-peek-failure" role="status">
          {failureCopy(unreadable)}
        </p>
      ) : null}

      {reply}
    </article>
  )
})
