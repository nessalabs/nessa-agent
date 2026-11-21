/**
 * The `TerminalSource` over the window's gateway client (`client.terminals`).
 *
 * ```text
 * follow ─▶ terminal.list ─▶ list, then each terminal.listed ─▶ onList
 * view.fit (first) ─▶ terminal.attach ─▶ terminal.output for (terminal, view) ─▶ the view's listeners
 * view.fit (later) ─▶ terminal.fit        view.focus ─▶ terminal.focus
 * the connection comes back, or a new client ─▶ list again, attach every view again at its fit
 * ```
 *
 * The gateway forgets a connection's views and its listing when the
 * connection goes, so every one is said again on the connection that
 * replaces it. Output is routed by terminal and view: one handler per
 * client, never one per view.
 */
import {
  NessaRpcError,
  type TerminalApi,
  type TerminalOutput,
  type TerminalSummary,
} from "@nessa/client"
import {
  TerminalOpenError,
  type TerminalFit,
  type TerminalOpenFailure,
  type TerminalProtocolMessage,
  type TerminalSource,
  type TerminalView,
} from "../../application/ports"
import type { TerminalEntry } from "../../model/terminal"
import { channelIdFrom, gatewayChannelId } from "./gateway-views"

/** What terminals ask of the window's gateway client. */
export interface TerminalClient {
  readonly terminals: TerminalApi
  on(event: "terminal.output", handler: (output: TerminalOutput) => void): () => void
  on(
    event: "terminal.listed",
    handler: (listed: { terminals: TerminalSummary[] }) => void,
  ): () => void
  onConnectionStateChange(handler: (state: { status: string }) => void): () => void
}

/** Every refusal `terminal.open` may answer, as the window says it. */
const openFailures: Readonly<Record<string, TerminalOpenFailure>> = {
  terminal_unavailable: "unavailable",
  terminal_limit: "limit",
  terminal_place_unsupported: "elsewhere",
  terminal_folder_missing: "folder-missing",
  terminal_start_failed: "start-failed",
  channel_not_found: "not-found",
  conversation_not_found: "not-found",
}

export function terminalEntry(summary: TerminalSummary): TerminalEntry {
  return {
    id: summary.terminalId,
    title: summary.title,
    kind: summary.kind,
    channelId: channelIdFrom(summary.channelId),
    ...(summary.conversationId === undefined
      ? {}
      : { conversationId: summary.conversationId }),
    requestId: summary.requestId,
    folder: summary.folder,
    shell: summary.shell,
    runsOn: summary.runsOn,
    startedAtMs: summary.startedAtMs,
    state: summary.state,
    ...(summary.exitStatus === undefined ? {} : { exitStatus: summary.exitStatus }),
    ...(summary.endedAtMs === undefined ? {} : { endedAtMs: summary.endedAtMs }),
  }
}

/** The gateway's words for a shell's channel: none for its default one. */
function shellChannel(channelId: string | undefined): { channelId?: string } {
  const id = channelId === undefined ? undefined : gatewayChannelId(channelId)
  return id === undefined ? {} : { channelId: id }
}

interface LiveView {
  readonly terminalId: string
  readonly viewId: string
  readonly listeners: Set<(message: TerminalProtocolMessage) => void>
  /** The last fit; none until the view is first measured, so it is not attached. */
  fit: TerminalFit | null
  /** Whether it took the keyboard last: said again on a new connection. */
  focused: boolean
}

export function terminalGateway(options: {
  readonly connected: () => Promise<TerminalClient>
  readonly newId: () => string
}): TerminalSource {
  const views = new Map<string, LiveView>()
  const followers = new Set<(terminals: readonly TerminalEntry[]) => void>()
  let bound: TerminalClient | null = null
  let unbind: (() => void) | null = null
  let binding: Promise<TerminalClient> | null = null

  const route = (terminalId: string, viewId: string) => `${terminalId} ${viewId}`
  const tell = (terminals: readonly TerminalSummary[]) => {
    const entries = terminals.map(terminalEntry)
    for (const follower of followers) follower(entries)
  }
  const warn = (what: string) => (error: unknown) =>
    console.warn(`A terminal ${what} was not taken`, error)

  /** Says everything this window holds on the gateway again, on `client`. */
  const restore = (client: TerminalClient) => {
    if (followers.size > 0) void client.terminals.list().then(tell, warn("list"))
    for (const view of views.values()) {
      if (!view.fit) continue
      const params = { terminalId: view.terminalId, viewId: view.viewId, ...view.fit }
      void client.terminals
        .attach(params)
        .then(() => (view.focused ? client.terminals.focus(params) : undefined))
        .catch(warn("view"))
    }
  }

  /** The client, listened on once: a new one is listened on and told everything again. */
  const client = (): Promise<TerminalClient> => {
    binding ??= options
      .connected()
      .then((next) => {
        if (next === bound) return next
        unbind?.()
        bound = next
        let lastStatus = "connected"
        const stops = [
          next.on("terminal.output", (output) => {
            const view = views.get(route(output.terminalId, output.viewId))
            if (!view) return
            let message: TerminalProtocolMessage
            try {
              message = JSON.parse(output.message) as TerminalProtocolMessage
            } catch {
              return
            }
            for (const listener of view.listeners) listener(message)
          }),
          next.on("terminal.listed", (listed) => tell(listed.terminals)),
          next.onConnectionStateChange((state) => {
            // The client recovered its socket: the gateway's side is new.
            if (state.status === "connected" && lastStatus !== "connected") restore(next)
            lastStatus = state.status
          }),
        ]
        unbind = () => stops.forEach((stop) => stop())
        restore(next)
        return next
      })
      .finally(() => {
        binding = null
      })
    return binding
  }

  // One line for everything the window says about terminals, each waiting
  // for the last one's answer: the gateway runs requests side by side, so
  // two in flight can land in either order. Keys reach the shell in the
  // order they were typed, and a view is attached before its first keys.
  let said: Promise<void> = Promise.resolve()
  const send = (what: string, act: (api: TerminalApi) => Promise<void>) => {
    said = said
      .then(() => client())
      .then((current) => act(current.terminals))
      .catch(warn(what))
  }

  // Keys typed while earlier ones are still on their way go together, as
  // one input, when their turn comes: the gateway answers each request on
  // its own, so only one input per terminal may be out at a time.
  const typed = new Map<string, Uint8Array[]>()
  const type = (terminalId: string, bytes: Uint8Array) => {
    const waiting = typed.get(terminalId)
    if (waiting) {
      waiting.push(bytes)
      return
    }
    typed.set(terminalId, [bytes])
    send("input", (api) => {
      const pieces = typed.get(terminalId) ?? []
      typed.delete(terminalId)
      return api.input(terminalId, joined(pieces))
    })
  }

  return {
    follow(onList) {
      followers.add(onList)
      // A new follower on a bound client lists now; binding lists for it.
      if (bound && !binding) void bound.terminals.list().then(tell, warn("list"))
      else void client().catch(warn("list"))
      return () => followers.delete(onList)
    },
    async open(request, requestId, fit) {
      let current: TerminalClient
      try {
        current = await client()
      } catch {
        throw new TerminalOpenError("unavailable")
      }
      try {
        const summary = await current.terminals.open({
          requestId,
          ...(request.kind === "shell"
            ? shellChannel(request.channelId)
            : { conversationId: request.conversationId, command: request.command }),
          cols: fit.cols,
          rows: fit.rows,
        })
        return terminalEntry(summary)
      } catch (error) {
        const code = error instanceof NessaRpcError ? error.code : undefined
        throw new TerminalOpenError((code && openFailures[code]) || "unavailable")
      }
    },
    view(terminalId): TerminalView {
      const view: LiveView = {
        terminalId,
        viewId: options.newId(),
        listeners: new Set(),
        fit: null,
        focused: false,
      }
      const key = route(terminalId, view.viewId)
      views.set(key, view)
      let released = false
      const params = () => ({
        terminalId,
        viewId: view.viewId,
        ...(view.fit as TerminalFit),
      })
      return {
        subscribe(listener) {
          view.listeners.add(listener)
          return () => view.listeners.delete(listener)
        },
        fit(fit) {
          if (released) return
          const first = view.fit === null
          view.fit = fit
          send("view", (api) => (first ? api.attach(params()) : api.fit(params())))
        },
        focus(fit) {
          if (released) return
          const first = view.fit === null
          view.fit = fit
          view.focused = true
          // Focus names a view the gateway holds: a view not shown yet is attached first.
          send("focus", async (api) => {
            if (first) await api.attach(params())
            await api.focus(params())
          })
        },
        input(bytes) {
          if (!released) type(terminalId, bytes)
        },
        history(start, count) {
          if (!released && view.fit)
            send("history", (api) => api.history(terminalId, view.viewId, start, count))
        },
        release() {
          if (released) return
          released = true
          views.delete(key)
          view.listeners.clear()
          if (view.fit) send("release", (api) => api.detach(terminalId, view.viewId))
        },
      }
    },
    input(terminalId, bytes) {
      type(terminalId, bytes)
    },
    async close(terminalId) {
      const current = await client()
      await current.terminals.close(terminalId)
    },
    async rename(terminalId, title) {
      const current = await client()
      await current.terminals.rename(terminalId, title)
    },
  }
}

function joined(pieces: readonly Uint8Array[]): Uint8Array {
  if (pieces.length === 1) return pieces[0] as Uint8Array
  const out = new Uint8Array(pieces.reduce((sum, piece) => sum + piece.length, 0))
  let at = 0
  for (const piece of pieces) {
    out.set(piece, at)
    at += piece.length
  }
  return out
}
