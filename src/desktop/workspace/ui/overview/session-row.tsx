import { memo, type KeyboardEvent as ReactKeyboardEvent } from "react"
import { useNow } from "../../adapters/dom/clock"
import { matchesChord } from "../../adapters/dom/shortcuts"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectSession } from "../../adapters/store/selectors"
import { sessionTime } from "../../model/time-labels"
import { agentName, agentOf } from "../../model/workspace-index"
import { AgentTile } from "../chrome/agent-tile"
import { StatusGlyph } from "../chrome/status-glyph"
import { overviewKeys } from "./overview-keys"
import { SessionPeek } from "./session-peek"
import type { OnAnswer } from "./settling"

/**
 * A session working, finished and not yet looked at, or earlier: its agent,
 * its title, the last thing it said, and — working — the turning glyph the
 * rest of the window shows, or else how long ago. A click peeks at it,
 * ↩ or a double-click opens it.
 */
export const SessionRow = memo(function SessionRow({
  sessionId,
  kind,
  current,
  selected,
  expanded,
  onFocus,
  onChoose,
  onOpen,
  onAnswer,
  onLeaveReply,
}: {
  sessionId: string
  kind: "working" | "finished" | "earlier"
  current: boolean
  /** The one the peek beside the list shows. */
  selected: boolean
  /** Showing its peek beneath it, where there is no room beside the list. */
  expanded: boolean
  onFocus: (sessionId: string) => void
  onChoose: (sessionId: string) => void
  onOpen: (sessionId: string) => void
  onAnswer: OnAnswer
  onLeaveReply: () => void
}) {
  const summary = useWorkspaceSelector((state) => selectSession(state, sessionId))
  const now = useNow(30_000)
  if (!summary) return null

  // ↩ opens, where a button would otherwise take it as a click: a click peeks.
  const onKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    const binding = overviewKeys.find((candidate) =>
      matchesChord(event.nativeEvent, candidate.chord),
    )
    if (binding?.command !== "open") return
    event.preventDefault()
    onOpen(sessionId)
  }

  return (
    <li className="agents-row-item" data-reflow={`row:${sessionId}`}>
      <button
        type="button"
        className="agents-row"
        data-kind={kind}
        data-overview-item={sessionId}
        data-selected={selected || undefined}
        aria-expanded={expanded}
        tabIndex={current ? 0 : -1}
        aria-label={`${summary.title}. ${agentName(agentOf(summary.model))}, ${kind}.`}
        onFocus={() => onFocus(sessionId)}
        onKeyDown={onKeyDown}
        onClick={() => onChoose(sessionId)}
        onDoubleClick={() => onOpen(sessionId)}
      >
        <AgentTile model={summary.model} size={24} />
        <span className="agents-row-text">
          <span className="agents-row-title agents-truncate">{summary.title}</span>
          <span className="agents-row-preview agents-truncate">{summary.preview}</span>
        </span>
        {kind === "working" ? (
          <StatusGlyph status="running" />
        ) : (
          <span className="agents-row-time">{sessionTime(summary.updatedAt, now)}</span>
        )}
      </button>
      {expanded ? (
        <div className="agents-inline-peek">
          <SessionPeek
            sessionId={sessionId}
            settling={undefined}
            onOpen={onOpen}
            onAnswer={onAnswer}
            onLeaveReply={onLeaveReply}
          />
        </div>
      ) : null}
    </li>
  )
})
