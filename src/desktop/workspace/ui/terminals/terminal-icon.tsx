/**
 * A terminal's leading icon: when Claude or Codex is running inside the
 * terminal (reported via OSC 7501 `app` or title fallback), shows its
 * rounded-square app badge; otherwise shows the standard terminal glyph.
 */
import { memo } from "react"
import { agentMarkDrawing } from "../../../../onboarding/ui/agent-mark"
import { DesktopIcon } from "../../../ui/icons"
import { tooltip } from "../../../ui/tooltip"
import {
  terminalAgentName,
  terminalStatusLine,
  type TerminalAgent,
  type TerminalProgramStatus,
} from "../../model/terminal"
import { StatusGlyph } from "../chrome/status-glyph"
import "./terminal-icon.css"

export const TerminalIcon = memo(function TerminalIcon({
  agent,
  size = 18,
}: {
  agent: TerminalAgent | null
  size?: number
}) {
  if (!agent) return <DesktopIcon name="terminal" size={size} aria-hidden />
  const drawing = agentMarkDrawing(agent)
  if (!drawing) return <DesktopIcon name="terminal" size={size} aria-hidden />
  return (
    <span
      className="workspace-terminal-agent-icon"
      data-terminal-agent={agent}
      aria-hidden="true"
      style={{ width: size, height: size }}
    >
      <svg viewBox="0 0 24 24" fillRule="evenodd" aria-hidden="true">
        <path d={drawing.path} fill="currentColor" />
      </svg>
    </span>
  )
})

/**
 * Compact status indicator for a terminal's active OSC 7501 state.
 * Matches Nessa's `StatusGlyph` vocabulary for blocked, working, and done,
 * and adds a red status point for `error`.
 */
export const TerminalStatusIndicator = memo(function TerminalStatusIndicator({
  status,
  decorative = false,
}: {
  status: TerminalProgramStatus | undefined
  decorative?: boolean
}) {
  if (!status) return null
  switch (status.state) {
    case "idle":
      return null
    case "blocked":
      return <StatusGlyph status="needs-you" decorative={decorative} />
    case "working":
      return <StatusGlyph status="running" decorative={decorative} />
    case "done":
      return <StatusGlyph status="unread" decorative={decorative} />
    case "error": {
      const label = terminalStatusLine(status)
      const says = decorative
        ? { "aria-hidden": true as const }
        : {
            role: "img",
            "aria-label": label,
            ...tooltip(label),
          }
      return (
        <span
          className="workspace-status workspace-terminal-error-dot"
          data-status="error"
          {...says}
        />
      )
    }
  }
})

/** Accessible label suffix when a recognized agent or OSC 7501 status is active. */
export function terminalStatusSummary(
  agent: TerminalAgent | null,
  status: TerminalProgramStatus | undefined,
): string | undefined {
  const parts: string[] = []
  if (agent) parts.push(terminalAgentName(agent))
  if (status && status.state !== "idle") parts.push(terminalStatusLine(status))
  return parts.length > 0 ? parts.join(" · ") : undefined
}
