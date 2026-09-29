import { memo, type RefObject } from "react"
import { DesktopIcon } from "../../../ui/icons"
import { tooltip } from "../../../ui/tooltip"
import { useNow } from "../../adapters/dom/clock"
import { labelOf } from "../../adapters/dom/shortcuts"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectAnswer,
  selectChannel,
  selectSession,
  selectTranscript,
  selectTranscriptFailure,
} from "../../adapters/store/selectors"
import { answering } from "../../application/workspace-state"
import { peekOf } from "../../model/overview/peek"
import { sessionTime } from "../../model/time-labels"
import { agentName, agentOf, modelName } from "../../model/workspace-index"
import { AgentTile } from "../chrome/agent-tile"
import { StatusGlyph } from "../chrome/status-glyph"
import { failureCopy } from "../failure-copy"
import { ApprovalActions, ApprovalCommand } from "../transcript/approval-request"
import { LiveRow } from "../transcript/live-row"
import { Message } from "../transcript/message"
import "../transcript/transcript.css"
import { overviewKeys } from "./overview-keys"
import { ReplyPill } from "./reply-pill"
import { answeredLabels, type OnAnswer, type Settling } from "./settling"

/**
 * A look into a session without leaving the overview. Under its title,
 * where it stands and — when the source says — what is going on, in a line
 * (`SessionSummary.now`; nothing when it says nothing). Then the turn's
 * story, top to bottom: the person's latest message, everything the agent
 * did and said since, in order, what it is doing now — and, when it waits on
 * the person, the request in full with its answers. The story is drawn with
 * the conversation's own pieces (its messages, the live row), so a session
 * reads the same here as in its pane, and a long turn scrolls within the
 * peek. A reply pill at its foot lets the person answer the agent from here.
 */
export const SessionPeek = memo(function SessionPeek({
  sessionId,
  settling,
  onOpen,
  onAnswer,
  onLeaveReply,
  replyRef,
}: {
  sessionId: string
  settling: Settling | undefined
  onOpen: (sessionId: string) => void
  onAnswer: OnAnswer
  /** Escape in the reply pill: the keyboard goes back to the list. */
  onLeaveReply: () => void
  replyRef?: RefObject<HTMLTextAreaElement | null>
}) {
  const live = useWorkspaceSelector((state) => selectSession(state, sessionId))
  const summary = settling?.summary ?? live
  // Read and kept by the workspace while the overview shows it (`onScreen`).
  const transcript = useWorkspaceSelector((state) => selectTranscript(state, sessionId))
  const unreadable = useWorkspaceSelector((state) =>
    selectTranscriptFailure(state, sessionId),
  )
  const channel = useWorkspaceSelector((state) =>
    summary ? selectChannel(state, summary.channelId)?.name : undefined,
  )
  const now = useNow(30_000)
  const waiting = settling !== undefined || live?.status === "needs-you"
  const approval = settling?.approval ?? (waiting ? (transcript?.approval ?? null) : null)
  const answer = useWorkspaceSelector((state) =>
    approval ? selectAnswer(state, sessionId, approval.id) : undefined,
  )
  if (!summary) return null
  const peek = transcript ? peekOf(transcript) : null
  // What the agent is doing now, while it works: the story's last line, where its reply will appear.
  const running = summary.status === "running" ? (peek?.activity ?? null) : null
  const answerable = approval !== null && !settling && !answering(answer, approval.id)
  const refused = settling ? undefined : answer?.failure
  const agent = agentName(agentOf(summary.model))
  const reply = (
    <ReplyPill
      sessionId={sessionId}
      agent={agent}
      fieldRef={replyRef}
      onLeave={onLeaveReply}
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
        <p className="agents-peek-state" data-status={summary.status}>
          <StatusGlyph status={waiting ? "needs-you" : summary.status} idle />
          <span>
            {waiting
              ? "Waiting for you"
              : summary.status === "running"
                ? "Working"
                : "Finished"}
          </span>
          <time dateTime={new Date(summary.updatedAt).toISOString()}>
            {sessionTime(summary.updatedAt, now)}
          </time>
        </p>
        {summary.now ? (
          <p className="agents-peek-summary agents-truncate" title={summary.now}>
            {summary.now}
          </p>
        ) : null}
      </div>

      {peek && (peek.asked || peek.since.length > 0 || running) ? (
        <section className="agents-peek-story" aria-label="This turn">
          {peek.asked ? (
            <Message sessionId={sessionId} message={peek.asked} isNew={false} />
          ) : null}
          {peek.since.map((message) => (
            <Message
              key={message.id}
              sessionId={sessionId}
              message={message}
              isNew={false}
            />
          ))}
          {running ? <LiveRow key="live" activity={running} /> : null}
        </section>
      ) : null}

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
            <p className="agents-peek-answered" data-answer={settling.choice}>
              <DesktopIcon name={settling.choice === "deny" ? "close" : "check"} />
              {answeredLabels[settling.choice]}
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
                  shortcut: labelOf(overviewKeys, "once"),
                }),
              }}
              onAnswer={(choice, at) => {
                if (answerable) onAnswer(summary, approval, choice, at)
              }}
            />
          )}
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
