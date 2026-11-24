/**
 * A look into a terminal running Claude or Codex from the Agents overview:
 * shows its badge, title, OSC 7501 status line, an Open button, and a live
 * view of the terminal itself so the person can inspect or reply directly.
 */
import { memo } from "react"
import { useNow } from "../../../../clock"
import { useWorkspaceSelector } from "../../adapters/store/hooks"
import { selectChannel, selectTerminal } from "../../adapters/store/selectors"
import {
  terminalAgentName,
  terminalAgentOf,
  terminalStatusLine,
} from "../../model/terminal"
import { sessionTime } from "../../model/time-labels"
import { TerminalIcon, TerminalStatusIndicator } from "../terminals/terminal-icon"
import { TerminalView } from "../terminals/terminal-view"

export const TerminalPeek = memo(function TerminalPeek({
  terminalId,
  placement,
  onOpen,
}: {
  terminalId: string
  placement: "beside" | "beneath"
  onOpen: (terminalId: string) => void
}) {
  const terminal = useWorkspaceSelector((state) => selectTerminal(state, terminalId))
  const channel = useWorkspaceSelector((state) =>
    terminal ? selectChannel(state, terminal.channelId)?.name : undefined,
  )
  const now = useNow(30_000)
  if (!terminal) return null
  const agent = terminalAgentOf(terminal)
  const status = terminal.programStatus
  const title = status?.title || terminal.title
  const subtitleParts = [
    agent ? `${terminalAgentName(agent)} in terminal` : "Terminal",
    channel ? `#${channel}` : undefined,
    sessionTime(status?.updatedAtMs ?? terminal.startedAtMs, now),
  ].filter(Boolean)
  return (
    <div className="agents-peek agents-terminal-peek" data-placement={placement}>
      <header className="agents-peek-header">
        <div className="agents-peek-heading">
          <TerminalIcon agent={agent} size={24} />
          <div className="agents-peek-titles">
            <h3 className="agents-peek-title">{title}</h3>
            <p className="agents-peek-meta">{subtitleParts.join(" · ")}</p>
          </div>
        </div>
        <button
          type="button"
          className="agents-peek-open"
          onClick={() => onOpen(terminal.id)}
        >
          Open
        </button>
      </header>
      {status && status.state !== "idle" ? (
        <p className="agents-terminal-peek-status" data-program-state={status.state}>
          <TerminalStatusIndicator status={status} decorative />
          <span>{terminalStatusLine(status)}</span>
        </p>
      ) : null}
      <div
        className="agents-terminal-peek-body"
        onKeyDown={(event) => {
          // Keys typed into the embedded terminal belong to the PTY, never to the overview list.
          event.stopPropagation()
        }}
      >
        <TerminalView terminalId={terminal.id} label={title} />
      </div>
    </div>
  )
})
