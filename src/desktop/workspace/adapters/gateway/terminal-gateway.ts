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
    ...(summary.programStatus === undefined
      ? {}
      : { programStatus: summary.programStatus }),
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
  /** A view whose first attach reached the client needs restoring on a new socket. */
  attachRequested: boolean
}

export function terminalGateway(options: {
  readonly connected: () => Promise<TerminalClient>
  readonly followClient?: (listener: (client: TerminalClient) => void) => () => void
  readonly newId: () => string
}): TerminalSource {
  const views = new Map<string, LiveView>()
  const focusedViews = new Map<string, LiveView>()
  const followers = new Set<(terminals: readonly TerminalEntry[]) => void>()
  let bound: TerminalClient | null = null
  let unbind: (() => void) | null = null
  let binding: Promise<TerminalClient> | null = null
  let pendingRestore: TerminalClient | null = null
  let stopFollowingClient: (() => void) | undefined
  let connectionRevision = 0

  const route = (terminalId: string, viewId: string) => `${terminalId} ${viewId}`
  const tell = (terminals: readonly TerminalSummary[]) => {
    const entries = terminals.map(terminalEntry)
    for (const follower of followers) follower(entries)
  }
  const warn = (what: string) => (error: unknown) =>
    console.warn(`A terminal ${what} was not taken`, error)
  const list = (client: TerminalClient) => {
    const revision = connectionRevision
    void client.terminals.list().then((entries) => {
      if (bound === client && revision === connectionRevision) tell(entries)
    }, warn("list"))
  }

  // The gateway runs requests side by side. Serialize view restoration with the
  // writes that follow it, including keys and any fits measured during restoration.
  let said: Promise<void> = Promise.resolve()
  const queue = (what: string, act: () => Promise<void>) => {
    said = said.then(act).catch(warn(what))
  }

  const restore = async (client: TerminalClient) => {
    if (pendingRestore !== client) return
    pendingRestore = null
    const revision = connectionRevision
    await Promise.all(
      Array.from(views.values(), async (view) => {
        if (!view.attachRequested || !view.fit) return
        const id = { terminalId: view.terminalId, viewId: view.viewId }
        try {
          await client.terminals.attach({ ...id, ...view.fit })
          if (
            revision === connectionRevision &&
            focusedViews.get(view.terminalId) === view
          ) {
            await client.terminals.focus({ ...id, ...view.fit })
          }
        } catch (error) {
          warn("view")(error)
        }
      }),
    )
  }

  const scheduleRestore = (client: TerminalClient) => {
    connectionRevision++
    if (followers.size > 0) list(client)
    pendingRestore = client
    queue("restore", () => restore(client))
  }

  /** A client may arrive from a terminal operation or be adopted by the workspace first. */
  const bind = (next: TerminalClient): TerminalClient => {
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
        if (state.status === "connected" && lastStatus !== "connected") {
          scheduleRestore(next)
        } else if (state.status !== "connected" && lastStatus === "connected") {
          connectionRevision++
          pendingRestore = null
        }
        lastStatus = state.status
      }),
    ]
    unbind = () => stops.forEach((stop) => stop())
    scheduleRestore(next)
    return next
  }

  const followClient = () => {
    stopFollowingClient ??= options.followClient?.(bind)
  }

  const stopFollowingIdleClient = () => {
    if (views.size > 0 || followers.size > 0) return
    stopFollowingClient?.()
    stopFollowingClient = undefined
  }

  const client = (): Promise<TerminalClient> => {
    binding ??= options
      .connected()
      .then(bind)
      .finally(() => {
        binding = null
      })
    return binding
  }

  const send = (what: string, act: (api: TerminalApi) => Promise<void>) => {
    queue(what, async () => {
      const current = await client()
      await restore(current)
      await act(current.terminals)
    })
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
      followClient()
      followers.add(onList)
      // A new follower on a bound client lists now; binding lists for it.
      if (bound && !binding) list(bound)
      else void client().catch(warn("list"))
      return () => {
        followers.delete(onList)
        stopFollowingIdleClient()
      }
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
      followClient()
      const view: LiveView = {
        terminalId,
        viewId: options.newId(),
        listeners: new Set(),
        fit: null,
        attachRequested: false,
      }
      const key = route(terminalId, view.viewId)
      views.set(key, view)
      let released = false
      const params = () => ({
        terminalId,
        viewId: view.viewId,
        ...(view.fit as TerminalFit),
      })
      const attach = (api: TerminalApi) => {
        view.attachRequested = true
        return api.attach(params())
      }
      return {
        subscribe(listener) {
          view.listeners.add(listener)
          return () => view.listeners.delete(listener)
        },
        fit(fit) {
          if (released) return
          view.fit = fit
          send("view", async (api) => {
            if (released) return
            if (view.attachRequested) await api.fit(params())
            else await attach(api)
          })
        },
        focus(fit) {
          if (released) return
          view.fit = fit
          focusedViews.set(terminalId, view)
          // Focus names a view the gateway holds: a view not shown yet is attached first.
          send("focus", async (api) => {
            if (released) return
            if (!view.attachRequested) await attach(api)
            if (!released) await api.focus(params())
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
          if (focusedViews.get(terminalId) === view) focusedViews.delete(terminalId)
          view.listeners.clear()
          if (view.attachRequested) {
            send("release", (api) => api.detach(terminalId, view.viewId))
          }
          stopFollowingIdleClient()
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
