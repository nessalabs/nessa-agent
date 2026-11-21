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
