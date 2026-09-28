import {
  memo,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from "react"
import { DesktopIcon } from "../../../ui/icons"
import { tooltip } from "../../../ui/tooltip"
import { useWorkspaceSelector } from "../../../workspace"
import { useNow } from "../../../workspace/adapters/dom/clock"
import { labelOf, matchesChord } from "../../../workspace/adapters/dom/shortcuts"
import type { WorkspaceFailureReason } from "../../../workspace/model/failure"
import {
  agentName,
  agentOf,
  type SessionSummary,
} from "../../../workspace/model/workspace-index"
import { sessionTime } from "../../../workspace/model/time-labels"
import type { Approval } from "../../../workspace/model/transcript"
import { AgentTile } from "../../../workspace/ui/chrome/agent-tile"
import { failureCopy } from "../../../workspace/ui/failure-copy"
import { selectSummary } from "../adapters/workspace-bridge"
import {
  answeredLabels,
  type Answer,
  type Request,
  type Settling,
} from "../model/request"
import { overviewKeys } from "./overview-keys"
import { SessionPeek } from "./session-peek"
import { useRequest } from "./use-request"

/**
 * One session waiting on the person, as a row of the list: its title, one
 * line of why, and the command itself — the only monospace on the page.
 * Deny and Allow wait at its end until the row is pointed at or has the
 * keyboard; holding ⌥ turns Allow into Always Allow, as ⌥ shows the other
 * choice in a Mac menu. Answered, the buttons give way to what became of it;
 * then the row fades, and the list closes over where it was.
 */
export const RequestRow = memo(function RequestRow({
  sessionId,
  current,
  selected,
  expanded,
  settling,
  failure,
  onFocus,
  onChoose,
  onOpen,
  onAnswer,
  onLeaveReply,
}: {
  sessionId: string
  /** The one the keyboard is on, or lands on when the list is entered. */
  current: boolean
  /** The one the peek beside the list shows. */
  selected: boolean
  /** Showing its peek beneath it, where there is no room beside the list. */
  expanded: boolean
  settling: Settling | undefined
  failure:
    { readonly approvalId: string; readonly reason: WorkspaceFailureReason } | undefined
  onFocus: (sessionId: string) => void
  /** Clicked, or Space: to peek at it. */
  onChoose: (sessionId: string) => void
  onOpen: (sessionId: string) => void
  onAnswer: (summary: SessionSummary, approval: Approval, choice: Answer) => void
  onLeaveReply: () => void
}) {
  const live = useWorkspaceSelector((state) => selectSummary(state, sessionId))
  const summary = settling?.summary ?? live
  const read = useRequest(sessionId, settling ? undefined : live?.revision)
  const request: Request = settling
    ? { kind: "approval", approval: settling.approval }
    : read
  const now = useNow(30_000)
  if (!summary) return null
  const agent = agentName(agentOf(summary.model))
  const approval = request.kind === "approval" ? request.approval : null
  const answerable = approval !== null && !settling
  const refused =
    failure && approval && failure.approvalId === approval.id ? failure.reason : null
  const unreadable = request.kind === "unreadable" ? request.reason : null
  const said = refused ?? unreadable

  const onKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    const binding = overviewKeys.find((candidate) =>
      matchesChord(event.nativeEvent, candidate.chord),
    )
    if (!binding) {
      if (event.code === "Space" && event.target === event.currentTarget) {
        event.preventDefault()
        onChoose(sessionId)
      }
      return
    }
    const command = binding.command
    if (command === "open") {
      // On a button, ↩ presses it; on the row itself, it opens the session.
      if (event.target !== event.currentTarget) return
      event.preventDefault()
      onOpen(sessionId)
      return
    }
    if (command !== "allow" && command !== "always" && command !== "deny") return
    event.preventDefault()
    event.stopPropagation()
    if (answerable) onAnswer(summary, approval, command)
  }

  // A click peeks at the session, a double-click opens it; the buttons answer.
  const onButton = (event: ReactMouseEvent<HTMLElement>) =>
    event.target instanceof Element && event.target.closest("button") !== null

  return (
    <li className="agents-row-item" data-reflow={`request:${sessionId}`}>
      <div
        className="agents-request"
        data-overview-item={sessionId}
        data-phase={settling?.phase}
        data-answer={settling?.answer}
        role="group"
        tabIndex={current ? 0 : -1}
        aria-label={`${summary.title}. ${agent} ${approval ? `wants to run ${approval.command}` : "is waiting for you"}.`}
        data-selected={selected || undefined}
        aria-expanded={expanded}
        onFocus={() => onFocus(sessionId)}
        onKeyDown={onKeyDown}
        onClick={(event) => {
          if (!onButton(event)) onChoose(sessionId)
        }}
        onDoubleClick={(event) => {
          if (!onButton(event)) onOpen(sessionId)
        }}
      >
        <AgentTile model={summary.model} size={24} />
        <span className="agents-row-text">
          <span className="agents-row-title agents-truncate">{summary.title}</span>
          <span className="agents-request-why agents-truncate">
            {said ? (
              <span className="agents-request-failure" role="status">
                {failureCopy(said)}
              </span>
            ) : approval ? (
              approval.reason
            ) : (
              summary.preview
            )}
          </span>
          {approval ? (
            <code
              className="agents-request-command agents-truncate"
              title={approval.command}
            >
              {approval.command}
            </code>
          ) : null}
        </span>
        <span className="agents-request-end">
          <time
            className="agents-row-time"
            dateTime={new Date(summary.updatedAt).toISOString()}
          >
            {sessionTime(summary.updatedAt, now)}
          </time>
          {approval ? (
            <span className="agents-request-actions">
              <button
                type="button"
                className="workspace-button agents-request-button"
                tabIndex={current && answerable ? 0 : -1}
                disabled={!answerable}
                {...tooltip("Don’t run it", { shortcut: labelOf(overviewKeys, "deny") })}
                onClick={() => {
                  if (answerable) onAnswer(summary, approval, "deny")
                }}
              >
                Deny
              </button>
              <button
                type="button"
                className="workspace-button agents-request-button"
                data-primary
                tabIndex={current && answerable ? 0 : -1}
                disabled={!answerable}
                {...tooltip("Run it once. Hold ⌥ to always allow it", {
                  shortcut: labelOf(overviewKeys, "allow"),
                })}
                onClick={(event) => {
                  if (answerable)
                    onAnswer(summary, approval, event.altKey ? "always" : "allow")
                }}
              >
                <span className="agents-request-once">Allow</span>
                <span className="agents-request-always" aria-hidden="true">
                  Always Allow
                </span>
              </button>
            </span>
          ) : request.kind === "question" ? (
            <span className="agents-request-actions">
              <button
                type="button"
                className="workspace-button agents-request-button"
                data-primary
                tabIndex={current ? 0 : -1}
                {...tooltip("Open the session to reply", {
                  shortcut: labelOf(overviewKeys, "open"),
                })}
                onClick={() => onOpen(sessionId)}
              >
                Reply
              </button>
            </span>
          ) : null}
          {settling ? (
            <span className="agents-request-settled" aria-hidden="true">
              <span className="agents-request-settled-mark">
                <DesktopIcon name={settling.answer === "deny" ? "close" : "check"} />
              </span>
              {answeredLabels[settling.answer]}
            </span>
          ) : null}
        </span>
      </div>
      {expanded ? (
        <div className="agents-inline-peek">
          <SessionPeek
            sessionId={sessionId}
            settling={settling}
            failure={failure}
            onOpen={onOpen}
            onAnswer={onAnswer}
            onLeaveReply={onLeaveReply}
          />
        </div>
      ) : null}
    </li>
  )
})
