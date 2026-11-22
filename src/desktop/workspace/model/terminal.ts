/** Active OSC 7501 program state reported by a terminal program. */
export type TerminalProgramState = "idle" | "working" | "done" | "blocked" | "error"

/** Reason a terminal program is blocked waiting on the person. */
export type TerminalBlockedKind = "permission" | "question" | "auth"

/** Most urgent active OSC 7501 program status record for a terminal. */
export interface TerminalProgramStatus {
  readonly state: TerminalProgramState
  readonly kind?: TerminalBlockedKind
  readonly progress?: number
  readonly app?: string
  readonly title?: string
  readonly msg?: string
  readonly updatedAtMs: number
}

/** Supported AI coding agents recognized inside a terminal. */
export type TerminalAgent = "claude" | "codex"

/**
 * One of the person's terminals as the window lists it: a chat in its
 * channel's session list that a pane shows as one real grid
 * (`docs/design/terminals.md`). The gateway owns the terminal and its list
 * (`terminal.listed`); the window keeps the last list it was given, whole.
 */
export interface TerminalEntry {
  readonly id: string
  /** The shell's name, or the first line of what Run runs. */
  readonly title: string
  /** shell: ⌘J or the + menu. run: a Run block in a reply. */
  readonly kind: "shell" | "run"
  /** The channel it belongs to, as the index names it. */
  readonly channelId: string
  /** The conversation whose Run started it. */
  readonly conversationId?: string
  /** The open request that started it, so a Run block finds its terminal. */
  readonly requestId: string
  readonly folder: string
  readonly shell: string
  /** Where it runs, in words: this Mac for now. */
  readonly runsOn: "here"
  readonly startedAtMs: number
  readonly state: "running" | "exited"
  readonly exitStatus?: number
  readonly endedAtMs?: number
  readonly programStatus?: TerminalProgramStatus
}

const agentByApp: Readonly<Record<string, TerminalAgent>> = {
  claude: "claude",
  "claude-code": "claude",
  codex: "codex",
  "openai-codex": "codex",
}

/** Recognize Claude or Codex from a terminal's OSC 7501 `app` (or fallback title app). */
export function terminalAgentOf(
  terminal: Pick<TerminalEntry, "programStatus"> | undefined,
): TerminalAgent | null {
  const app = terminal?.programStatus?.app?.toLowerCase()
  if (!app || !Object.hasOwn(agentByApp, app)) return null
  return agentByApp[app]
}

const agentNames: Readonly<Record<TerminalAgent, string>> = {
  claude: "Claude",
  codex: "Codex",
}

/** Display name for a recognized terminal agent. */
export function terminalAgentName(agent: TerminalAgent): string {
  return agentNames[agent]
}

const blockedKindCopy: Readonly<Record<TerminalBlockedKind, string>> = {
  permission: "Needs permission",
  question: "Has a question",
  auth: "Needs sign-in",
}

/** Plain-language one-line description of a terminal's OSC 7501 status. */
export function terminalStatusLine(status: TerminalProgramStatus): string {
  if (status.msg) return status.msg
  switch (status.state) {
    case "blocked":
      return status.kind ? blockedKindCopy[status.kind] : "Waiting on you"
    case "working":
      return status.progress !== undefined ? `Working · ${status.progress}%` : "Working"
    case "done":
      return "Finished"
    case "error":
      return "Failed"
    case "idle":
      return "Idle"
  }
}

/**
 * The terminal's effective OSC 7501 status for display and activity rolls:
 * once a terminal has exited, live states (`working`, `blocked`) no longer
 * apply while settled states (`done`, `error`, `idle`) remain visible.
 */
export function effectiveTerminalStatus(
  terminal: Pick<TerminalEntry, "state" | "programStatus"> | undefined,
): TerminalProgramStatus | undefined {
  const status = terminal?.programStatus
  if (!status) return undefined
  if (
    terminal.state === "exited" &&
    (status.state === "working" || status.state === "blocked")
  ) {
    return undefined
  }
  return status
}

/**
 * Whether a terminal should appear in the Agents overview: a recognized AI
 * agent (Claude or Codex) that is running, or has a settled `done`/`error`
 * result waiting for the person.
 */
export function isOverviewTerminalAgent(terminal: TerminalEntry): boolean {
  if (terminalAgentOf(terminal) === null) return false
  if (terminal.state === "running") return true
  const state = effectiveTerminalStatus(terminal)?.state
  return state === "done" || state === "error"
}

/** Which Agents overview group a terminal agent belongs in by its OSC 7501 state. */
export function terminalOverviewGroup(
  terminal: Pick<TerminalEntry, "programStatus">,
): "needsYou" | "working" | "finished" | "earlier" {
  switch (terminal.programStatus?.state) {
    case "blocked":
      return "needsYou"
    case "working":
      return "working"
    case "done":
    case "error":
      return "finished"
    default:
      return "earlier"
  }
}

/** The list as the gateway last sent it, by id, in the order it sent it. */
export function terminalsById(
  list: readonly TerminalEntry[],
): Readonly<Record<string, TerminalEntry>> {
  return Object.fromEntries(list.map((terminal) => [terminal.id, terminal]))
}

/** A channel's shells, newest first, as its session list shows them. */
export function channelShells(
  terminals: Readonly<Record<string, TerminalEntry>>,
  channelId: string,
): TerminalEntry[] {
  return Object.values(terminals)
    .filter((terminal) => terminal.kind === "shell" && terminal.channelId === channelId)
    .sort((a, b) => b.startedAtMs - a.startedAtMs)
}
