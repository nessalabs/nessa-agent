import { memo, type KeyboardEvent as ReactKeyboardEvent } from "react"
import { useWorkspaceSelector } from "../../../workspace"
import { useNow } from "../../../workspace/adapters/dom/clock"
import { matchesChord } from "../../../workspace/adapters/dom/shortcuts"
import {
  agentName,
  agentOf,
  type SessionSummary,
} from "../../../workspace/model/overview"
import { sessionTime } from "../../../workspace/model/time-labels"
import type { Approval } from "../../../workspace/model/transcript"
import { AgentTile } from "../../../workspace/ui/chrome/agent-tile"
import { StatusGlyph } from "../../../workspace/ui/chrome/status-glyph"
import { selectSummary } from "../adapters/workspace-bridge"
import type { Answer } from "../model/request"
import { overviewKeys } from "./overview-keys"
import { SessionPeek } from "./session-peek"

/**
 * A session working, or finished and not yet looked at: its agent, its
 * title, the last thing it said, and — working — the turning glyph the rest
 * of the window shows, or — finished — how long ago. A click peeks at it,
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
}: {
  sessionId: string
  kind: "working" | "finished"
  current: boolean
  /** The one the peek beside the list shows. */
  selected: boolean
  /** Showing its peek beneath it, where there is no room beside the list. */
  expanded: boolean
  onFocus: (sessionId: string) => void
  onChoose: (sessionId: string) => void
  onOpen: (sessionId: string) => void
  onAnswer: (summary: SessionSummary, approval: Approval, choice: Answer) => void
}) {
  const summary = useWorkspaceSelector((state) => selectSummary(state, sessionId))
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
            failure={undefined}
            onOpen={onOpen}
            onAnswer={onAnswer}
          />
        </div>
      ) : null}
    </li>
  )
})
