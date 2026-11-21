import type { RpcRequester } from "../application/session-port.js"
import { ProductMethod } from "../generated/product.js"
import type {
  TerminalOpenParams,
  TerminalSummary,
  TerminalViewParams,
} from "../generated/product.js"

/**
 * The person's terminals (`terminal.*`): real shells on the gateway's machine,
 * shown in views. The gateway lays the screen out and sends each view its
 * rows as `terminal.output` events; a view paints them and sends keys back.
 *
 * Only the owner's own window may call these. An agent never reaches a
 * terminal, and a request never names a folder: a shell starts in its
 * channel's folder, a Run where its conversation's agent works.
 *
 * Several views may show one terminal. The view the person is typing in sets
 * the size (`focus`); the others report their fit (`fit`) and scale.
 */
export type TerminalApi = {
  /** Start a terminal. The same `requestId` again answers the terminal it started. */
  open(params: TerminalOpenParams): Promise<TerminalSummary>
  /** The person's terminals, oldest first; afterwards `terminal.listed` follows each change. */
  list(): Promise<TerminalSummary[]>
  /** Show a terminal in a view: a checkpoint of its screen, then live output. */
  attach(params: TerminalViewParams): Promise<void>
  /** The view took the keyboard: the terminal takes its fit. */
  focus(params: TerminalViewParams): Promise<void>
  /** The view's box changed; it resizes the terminal only while the view owns the size. */
  fit(params: TerminalViewParams): Promise<void>
  /** Stop showing a terminal in a view. It keeps running. */
  detach(terminalId: string, viewId: string): Promise<void>
  /** Bytes for the terminal's program. */
  input(terminalId: string, bytes: Uint8Array): Promise<void>
  /** Retained rows for one view, answered as a `history` message on that view. */
  history(terminalId: string, viewId: string, start: number, count: number): Promise<void>
  /** End a terminal: hung up, then killed after 5 s, and gone from the list. */
  close(terminalId: string): Promise<void>
  /** Give a terminal the person's own name (1 to 80 characters); every list shows it. */
  rename(terminalId: string, title: string): Promise<void>
}

/** The largest input the gateway takes in one call, before base64. */
export const terminalInputLimit = 32 * 1024

export function createTerminalApi(session: RpcRequester): TerminalApi {
  const done = async (method: string, params: unknown) => {
    await session.request(method, params)
  }
  return {
    open: async (params) => {
      const answer = await session.request(ProductMethod.TerminalOpen, params)
      if (!isRecord(answer) || !isSummary(answer.terminal))
        throw new Error("Invalid terminal open response")
      return answer.terminal
    },
    list: async () => {
      const answer = await session.request(ProductMethod.TerminalList, {})
      if (
        !isRecord(answer) ||
        !Array.isArray(answer.terminals) ||
        !answer.terminals.every(isSummary)
      )
        throw new Error("Invalid terminal list response")
      return answer.terminals
    },
    attach: (params) => done(ProductMethod.TerminalAttach, params),
    focus: (params) => done(ProductMethod.TerminalFocus, params),
    fit: (params) => done(ProductMethod.TerminalFit, params),
    detach: (terminalId, viewId) =>
      done(ProductMethod.TerminalDetach, { terminalId, viewId }),
    input: async (terminalId, bytes) => {
      // One call per piece, in order: a large paste stays under the gateway's limit.
      for (let at = 0; at < bytes.length || at === 0; at += terminalInputLimit) {
        await done(ProductMethod.TerminalInput, {
          terminalId,
          data: base64(bytes.subarray(at, at + terminalInputLimit)),
        })
        if (bytes.length === 0) break
      }
    },
    history: (terminalId, viewId, start, count) =>
      done(ProductMethod.TerminalHistory, { terminalId, viewId, start, count }),
    close: (terminalId) => done(ProductMethod.TerminalClose, { terminalId }),
    rename: (terminalId, title) =>
      done(ProductMethod.TerminalRename, { terminalId, title }),
  }
}

function base64(bytes: Uint8Array): string {
  let binary = ""
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary)
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value)
}

/** A summary the window can rely on: the fields it reads are there and typed. */
export function isSummary(value: unknown): value is TerminalSummary {
  return (
    isRecord(value) &&
    typeof value.terminalId === "string" &&
    typeof value.title === "string" &&
    (value.kind === "shell" || value.kind === "run") &&
    typeof value.requestId === "string" &&
    typeof value.folder === "string" &&
    typeof value.shell === "string" &&
    typeof value.startedAtMs === "number" &&
    (value.state === "running" || value.state === "exited") &&
    (value.channelId === undefined || typeof value.channelId === "string") &&
    (value.conversationId === undefined || typeof value.conversationId === "string") &&
    (value.exitStatus === undefined || typeof value.exitStatus === "number") &&
    (value.endedAtMs === undefined || typeof value.endedAtMs === "number")
  )
}
