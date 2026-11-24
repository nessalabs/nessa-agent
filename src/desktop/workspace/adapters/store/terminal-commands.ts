/**
 * Terminals as the window's commands: ⌘J and the + menu open a shell in the
 * channel in front, a pane's close ends one. The gateway owns each terminal
 * and its list; these ask it, then show what it answered.
 */
import { TerminalOpenError, type TerminalFit } from "../../application/ports"
import { focusedChannel, paneShowingItem } from "../../application/workspace-state"
import { terminalItem } from "../../model/pane-item"
import { terminalsById, type TerminalEntry } from "../../model/terminal"
import type { WorkspaceCommand } from "./commands"
import { workspaceActions } from "./slice"

/** The fit a terminal starts at, before any view has measured itself. */
const startFit: TerminalFit = { cols: 100, rows: 30 }

/**
 * Opens a new shell in `channelId` — the channel in front when none is
 * named — and shows it in the focused pane, as a session opens. Resolves
 * the terminal's id, or `undefined` beside no terminals. Rejects with a
 * `TerminalOpenError` the caller shows.
 */
export function newTerminal(
  options: { channelId?: string; beside?: boolean } = {},
): WorkspaceCommand<Promise<string | undefined>> {
  return async (dispatch, getState, { terminals, newId }) => {
    if (!terminals) return undefined
    const state = getState().workspace
    const channelId =
      options.channelId || focusedChannel(state) || state.view.channelId || undefined
    const entry = await terminals.open({ kind: "shell", channelId }, newId(), startFit)
    dispatch(listedNow(entry))
    dispatch(
      options.beside
        ? openTerminalBeside({ terminalId: entry.id })
        : workspaceActions.openTerminal({ terminalId: entry.id }),
    )
    return entry.id
  }
}

/**
 * Opens a terminal beside the focused pane, as `openBeside` opens a session:
 * where the room the page measures allows, else in the pane's place.
 */
export function openTerminalBeside(options: {
  terminalId: string
  replace?: boolean
}): WorkspaceCommand {
  return (dispatch, _getState, { measure }) => {
    dispatch(workspaceActions.openedTerminalBeside({ ...options, room: measure() }))
  }
}

/**
 * Runs a reply's command in a new terminal in its conversation's folder, as
 * a Run block's button does. Resolves the terminal, or `undefined` beside no
 * terminals. Rejects with a `TerminalOpenError` the block shows.
 */
export function runInConversation(options: {
  conversationId: string
  command: string
  requestId: string
  fit: TerminalFit
}): WorkspaceCommand<Promise<TerminalEntry | undefined>> {
  return async (dispatch, _getState, { terminals }) => {
    if (!terminals) return undefined
    const entry = await terminals.open(
      { kind: "run", conversationId: options.conversationId, command: options.command },
      options.requestId,
      options.fit,
    )
    dispatch(listedNow(entry))
    return entry
  }
}

/** Sends Control-C to a terminal: a Run block's stop. */
export function interruptTerminal(terminalId: string): WorkspaceCommand<void> {
  return (_dispatch, _getState, { terminals }) => {
    terminals?.input(terminalId, new Uint8Array([3]))
  }
}

/**
 * The list with a terminal the window just opened, before the gateway's
 * next listing says so, so what opened it can show it at once.
 */
function listedNow(entry: TerminalEntry): WorkspaceCommand<void> {
  return (dispatch, getState) => {
    const listed = getState().workspace.terminals
    if (!Object.hasOwn(listed, entry.id))
      dispatch(
        workspaceActions.terminalsListed({
          terminals: { ...listed, ...terminalsById([entry]) },
        }),
      )
  }
}

/**
 * Names a terminal as the person typed it. The list changes when the gateway
 * announces it, so every window shows the same name.
 */
export function renameTerminal(
  terminalId: string,
  title: string,
): WorkspaceCommand<Promise<void>> {
  return async (_dispatch, _getState, { terminals }) => {
    const name = title.trim()
    if (!terminals || !name) return
    try {
      await terminals.rename(terminalId, name.slice(0, 80))
    } catch (error) {
      console.warn("A terminal was not renamed", error)
    }
  }
}

/** Ends a terminal and closes the pane that shows it, if one does. */
export function closeTerminal(terminalId: string): WorkspaceCommand<Promise<void>> {
  return async (dispatch, getState, { terminals, newId }) => {
    const panes = getState().workspace.panes
    const pane = panes ? paneShowingItem(panes, terminalItem(terminalId)) : undefined
    if (pane) dispatch(workspaceActions.paneClosed({ pane: pane.key, draftId: newId() }))
    if (!terminals) return
    try {
      await terminals.close(terminalId)
    } catch (error) {
      // Already gone is closed; anything else stays listed and can be closed again.
      if (!(error instanceof TerminalOpenError))
        console.warn("A terminal did not close", error)
    }
  }
}
