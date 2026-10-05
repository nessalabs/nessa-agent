import {
  memo,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from "react"
import { DesktopIcon } from "../../../ui/icons"
import { tooltip } from "../../../ui/tooltip"
import { useNow } from "../../adapters/dom/clock"
import { isMac } from "../../../adapters/platform"
import { matchesChord } from "../../../model/keyboard"
import { labelOf } from "../../adapters/dom/shortcuts"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import {
  selectAnswer,
  selectSession,
  selectTranscript,
  selectTranscriptFailure,
} from "../../adapters/store/selectors"
import { answering } from "../../application/workspace-state"
import { requestOf, type Request } from "../../model/overview/request"
import { sessionTime } from "../../model/time-labels"
import { agentName, agentOf } from "../../model/workspace-index"
import { AgentTile } from "../chrome/agent-tile"
import { failureCopy, readFailureCopy } from "../failure-copy"
import { offersChoice, optionOf } from "../../model/transcript"
import { approvalAsker } from "../transcript/approval-request"
import { overviewKeys } from "./overview-keys"
import { SessionPeek } from "./session-peek"
import { answeredLabels, type OnAnswer, type Settling } from "./settling"

/**
 * One session waiting on the person, as a row of the list: its title, one
 * line of why, and the command itself — the only monospace on the page.
 * The review's answers wait at its end until the row is pointed at or has
 * the keyboard. Where the review offers always as well as once, holding ⌥
 * turns the once button into that answer, as ⌥ shows the other choice in a
 * Mac menu. Answered, the buttons give way to what became of it;
 * then the row fades, and the list closes over where it was.
 */
export const RequestRow = memo(function RequestRow({
  sessionId,
  current,
  selected,
  expanded,
  settling,
  onFocus,
  onChoose,
  onOpen,
  onAnswer,
  onLeaveReply,
  takesKey,
}: {
  sessionId: string
  /** The one the keyboard is on, or lands on when the list is entered. */
  current: boolean
  /** The one the peek beside the list shows. */
  selected: boolean
  /** Showing its peek beneath it, where there is no room beside the list. */
  expanded: boolean
  settling: Settling | undefined
  onFocus: (sessionId: string) => void
  /** Clicked, or Space: to peek at it. */
  onChoose: (sessionId: string) => void
  onOpen: (sessionId: string) => void
  onAnswer: OnAnswer
  onLeaveReply: () => void
  /** Whether a key press may answer or open (`takesAnswerKey`). */
  takesKey: (event: KeyboardEvent) => boolean
}) {
  const live = useWorkspaceSelector((state) => selectSession(state, sessionId))
  const summary = settling?.summary ?? live
  const transcript = useWorkspaceSelector((state) => selectTranscript(state, sessionId))
  const readFailure = useWorkspaceSelector((state) =>
    selectTranscriptFailure(state, sessionId),
  )
  const request: Request = settling
    ? { kind: "approval", approval: settling.approval }
    : requestOf(transcript, readFailure)
  const approval = request.kind === "approval" ? request.approval : null
  // The workspace's answer to it: on its way (from here or a pane), or refused with why.
  const answer = useWorkspaceSelector((state) =>
    approval ? selectAnswer(state, sessionId, approval.id) : undefined,
  )
  const now = useNow(30_000)
  if (!summary) return null
  const agent = agentName(agentOf(summary.model))
  const answerable = approval !== null && !settling && !answering(answer, approval.id)
  const refused = settling ? undefined : answer?.failure
  const unreadable = request.kind === "unreadable" ? request.reason : null
  const denies = approval?.options.filter((option) => option.choice === "deny") ?? []
  const onceOptions = approval?.options.filter((option) => option.choice === "once") ?? []
  // Always folds into the first once button, where the review offers both.
  const foldedAlways =
    onceOptions.length > 0
      ? approval?.options.find((option) => option.choice === "always")
      : undefined
  const standaloneAlways =
    approval?.options.filter(
      (option) => option.choice === "always" && option !== foldedAlways,
    ) ?? []

  const onKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    const binding = overviewKeys.find((candidate) =>
      matchesChord(event.nativeEvent, candidate.chord, isMac),
    )
    if (!binding) {
      if (event.code === "Space" && event.target === event.currentTarget) {
        event.preventDefault()
        onChoose(sessionId)
      }
      return
    }
    const command = binding.command
    if (
      command === "open" ||
      command === "once" ||
      command === "always" ||
      command === "deny"
    ) {
      // Only a fresh press, made after the keyboard came here, answers or opens.
      if (!takesKey(event.nativeEvent)) {
        event.preventDefault()
        event.stopPropagation()
        return
      }
    }
    if (command === "open") {
      // On a button, ↩ presses it; on the row itself, it opens the session.
      if (event.target !== event.currentTarget) return
      event.preventDefault()
      onOpen(sessionId)
      return
    }
    if (command !== "once" && command !== "always" && command !== "deny") return
    event.preventDefault()
    event.stopPropagation()
    // A chord names a choice, so it answers the first option of that choice.
    const chosen = optionOf(approval, command)
    if (answerable && chosen) onAnswer(summary, approval, chosen, event.timeStamp)
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
        data-answer={settling?.choice}
        role="group"
        tabIndex={current ? 0 : -1}
        aria-label={`${summary.title}. ${approval ? `${approvalAsker(approval.origin, agent)} wants to run ${approval.command}` : `${agent} is waiting for you`}.`}
        data-offers-always={
          approval && offersChoice(approval, "always") ? true : undefined
        }
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
            {refused ? (
              <span className="agents-request-failure" role="status">
                {failureCopy(refused)}
              </span>
            ) : unreadable ? (
              <span className="agents-request-failure" role="status">
                {readFailureCopy(unreadable, "conversation")}
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
              {denies.map((option) => (
                <button
                  key={option.id}
                  type="button"
                  className="workspace-button agents-request-button"
                  data-answer={option.choice}
                  tabIndex={current && answerable ? 0 : -1}
                  disabled={!answerable}
                  {...tooltip("Don’t run it", {
                    shortcut: labelOf(overviewKeys, "deny"),
                  })}
                  onClick={(event) => {
                    if (answerable) onAnswer(summary, approval, option, event.timeStamp)
                  }}
                >
                  {option.label}
                </button>
              ))}
              {onceOptions.map((option) => (
                <button
                  key={option.id}
                  type="button"
                  className="workspace-button agents-request-button"
                  data-answer={option.choice}
                  data-primary
                  tabIndex={current && answerable ? 0 : -1}
                  disabled={!answerable}
                  {...tooltip(
                    foldedAlways && option === onceOptions[0]
                      ? "Run it once. Hold ⌥ to always allow it"
                      : option.label,
                    { shortcut: labelOf(overviewKeys, "once") },
                  )}
                  onClick={(event) => {
                    if (!answerable) return
                    const chosen =
                      foldedAlways && option === onceOptions[0] && event.altKey
                        ? foldedAlways
                        : option
                    onAnswer(summary, approval, chosen, event.timeStamp)
                  }}
                >
                  <span className="agents-request-once">{option.label}</span>
                  {foldedAlways && option === onceOptions[0] ? (
                    <span className="agents-request-always" aria-hidden="true">
                      {foldedAlways.label}
                    </span>
                  ) : null}
                </button>
              ))}
              {standaloneAlways.map((option) => (
                <button
                  key={option.id}
                  type="button"
                  className="workspace-button agents-request-button"
                  data-answer={option.choice}
                  data-primary
                  tabIndex={current && answerable ? 0 : -1}
                  disabled={!answerable}
                  {...tooltip(option.label, {
                    shortcut: labelOf(overviewKeys, "always"),
                  })}
                  onClick={(event) => {
                    if (answerable) onAnswer(summary, approval, option, event.timeStamp)
                  }}
                >
                  {option.label}
                </button>
              ))}
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
                <DesktopIcon name={settling.choice === "deny" ? "close" : "check"} />
              </span>
              {answeredLabels[settling.choice]}
            </span>
          ) : null}
        </span>
      </div>
      {expanded ? (
        <div className="agents-inline-peek">
          <SessionPeek
            sessionId={sessionId}
            placement="beneath"
            settling={settling}
            onOpen={onOpen}
            onAnswer={onAnswer}
            onLeaveReply={onLeaveReply}
          />
        </div>
      ) : null}
    </li>
  )
})
