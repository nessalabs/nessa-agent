/**
 * A terminal running Claude or Codex listed in the Agents overview: its
 * Claude/Codex badge, title, OSC 7501 status line, and trailing state or time.
 * Click peeks at it; Enter or double-click opens the terminal pane.
 */
import { memo, type KeyboardEvent as ReactKeyboardEvent } from "react"
import { useNow } from "../../../../clock"
import { isMac } from "../../../adapters/platform"
import { matchesChord } from "../../../model/keyboard"
import { ListRow } from "../../../ui/list-row"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectTerminal } from "../../adapters/store/selectors"
import {
  terminalAgentName,
  terminalAgentOf,
  terminalStatusLine,
} from "../../model/terminal"
import { sessionTime } from "../../model/time-labels"
import { TerminalIcon, TerminalStatusIndicator } from "../terminals/terminal-icon"
import { overviewKeys } from "./overview-keys"
import { TerminalPeek } from "./terminal-peek"

export const TerminalOverviewRow = memo(function TerminalOverviewRow({
  terminalId,
  kind,
  current,
  selected,
  expanded,
  onFocus,
  onChoose,
  onOpen,
  takesKey,
}: {
  terminalId: string
  kind: "needsYou" | "working" | "finished" | "earlier"
  current: boolean
  selected: boolean
  expanded: boolean
  onFocus: (id: string) => void
  onChoose: (id: string) => void
  onOpen: (terminalId: string) => void
  takesKey: (event: KeyboardEvent) => boolean
}) {
  const terminal = useWorkspaceSelector((state) => selectTerminal(state, terminalId))
  const now = useNow(30_000)
  if (!terminal) return null
  const agent = terminalAgentOf(terminal)
  const status = terminal.programStatus
  const title = status?.title || terminal.title
  const description =
    status && status.state !== "idle"
      ? terminalStatusLine(status)
      : agent
        ? `${terminalAgentName(agent)} in terminal`
        : "Terminal"
  const updatedAt = status?.updatedAtMs ?? terminal.startedAtMs

  const onKeyDown = (event: ReactKeyboardEvent<HTMLElement>) => {
    const binding = overviewKeys.find((candidate) =>
      matchesChord(event.nativeEvent, candidate.chord, isMac),
    )
    if (binding?.command !== "open") return
    event.preventDefault()
    if (!takesKey(event.nativeEvent)) {
      event.stopPropagation()
      return
    }
    onOpen(terminalId)
  }

  return (
    <li className="agents-row-item">
      <ListRow
        as="button"
        className="agents-row agents-terminal-row"
        data-kind={kind}
        data-overview-item={terminalId}
        data-overview-terminal=""
        data-terminal-agent={agent ?? undefined}
        data-program-state={status?.state}
        selected={selected}
        unread={kind === "finished"}
        aria-expanded={expanded}
        tabIndex={current ? 0 : -1}
        aria-label={`${title}. ${agent ? `${terminalAgentName(agent)} in terminal` : "Terminal"}, ${description}.`}
        onFocus={() => onFocus(terminalId)}
        onKeyDown={onKeyDown}
        onClick={() => onChoose(terminalId)}
        onDoubleClick={() => onOpen(terminalId)}
        leading={<TerminalIcon agent={agent} size={24} />}
        title={title}
        description={description}
        trailing={
          kind === "needsYou" || kind === "working" || status?.state === "error" ? (
            <TerminalStatusIndicator status={status} />
          ) : (
            <span className="agents-row-time">{sessionTime(updatedAt, now)}</span>
          )
        }
      />
      {expanded ? (
        <div className="agents-inline-peek">
          <TerminalPeek terminalId={terminalId} placement="beneath" onOpen={onOpen} />
        </div>
      ) : null}
    </li>
  )
})
