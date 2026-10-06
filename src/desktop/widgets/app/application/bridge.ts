/**
 * The `ui/*` bridge for one app view (MCP Apps 2026-01-26): the host side of
 * the conversation with one app's frame, from reading its UI resource to its
 * end. It carries out the lifecycle (`model/lifecycle.ts`, the design table
 * on #349), answers the app's requests through the ports, and tells the app
 * of its tool call and host context — never before the app said
 * `ui/notifications/initialized`.
 *
 * It knows nothing of the DOM: what reaches it is already a typed message
 * (`model/messages.ts`) read from a frame the transport vouched for
 * (`adapters/dom/frame-transport.ts`), and what it sends goes to `post`. The
 * address its server calls go to — the conversation, the server, the call and
 * this mount of it — is the view's own, never anything the app says. Each
 * bridge is one mount: it mints its `instanceId` when it is made, and releases
 * that mount once, the first time the view fails or ends (#384, M1–M7).
 */
import { appDocument, approvedDomains, cspPolicy } from "../model/csp"
import { sandboxMethods } from "../model/sandbox-methods"
import { appHostContext, changedContext, inlineMaxHeight } from "../model/host-context"
import {
  errorCodes,
  notify,
  refuse,
  relayError,
  reply,
  request,
  type JsonObject,
  type Outgoing,
  type RequestId,
} from "../model/json-rpc"
import {
  advance,
  firstState,
  frameOn,
  type Lifecycle,
  type LifecycleEffect,
  type LifecycleEvent,
} from "../model/lifecycle"
import {
  contentModalities,
  type AppRequest,
  type FromFrame,
  type Initialize,
} from "../model/messages"
import { requestMode } from "../model/places"
import { uiResource, type UiResource } from "../model/resource"
import {
  callView,
  nothingTold,
  toolNotifications,
  type AppCall,
  type CallTold,
} from "../model/tool-call"
import { blockedOrigins, firstView, type AppViewState } from "../model/app-view"
import type { HostContext, OpenPlace, WidgetPlace } from "../../model/widget-state"
import type { AppAddress, ConversationAnswer, McpAppPorts, ServerAnswer } from "./ports"

/** The protocol version this host speaks. */
export const protocolVersion = "2026-01-26"

/** How long the bridge waits, in milliseconds, before it gives up on each stage. */
export const deadlines = {
  /** For the sandbox proxy to say it is ready. */
  proxy: 15_000,
  /** For the app to say `ui/initialize`, and then `initialized`. */
  initialize: 15_000,
  /** For the app to answer `ui/resource-teardown`. */
  teardown: 3_000,
  /** Before the app's resource is read again, when the gateway was too busy to read it (L1b). */
  retryRead: 1_000,
  /** For a port to answer one of the app's requests; `tools/call` waits the server's `callWithin`. */
  request: 60_000,
} as const

/** At most this many of an app's requests wait on the host at once. */
export const pendingLimit = 16

/**
 * How many times the app's resource is read while the gateway answers that it
 * is busy, `deadlines.retryRead` apart, before the view fails (L1b).
 */
export const busyReads = 15

/** The id of the host's one request to a view: its teardown. */
export const teardownId = "nessa-teardown"

export interface BridgeOptions {
  readonly place: WidgetPlace
  readonly server: string
  readonly call: AppCall
  readonly context: HostContext
  readonly ports: McpAppPorts
  /** Sends a message to the frame; called only while the frame is on the page. */
  readonly post: (message: Outgoing) => void
  /** The host's callbacks for the view's place. */
  readonly host: { open(place: OpenPlace): void; close(): void }
  /** Told what the view shows when the bridge is made, and of each change. */
  readonly onView: (view: AppViewState) => void
}

export interface AppBridge {
  /** One message from the frame, already read and vouched for. */
  receive(message: FromFrame): void
  /** The tool call, as it changes. */
  setCall(call: AppCall): void
  /** The place's context, as it changes. */
  setContext(context: HostContext): void
  /** The view's place was removed: nothing is sent to it again. */
  remove(): void
  /** What the view shows now. */
  view(): AppViewState
}

const logLimit = 100

export function createAppBridge(options: BridgeOptions): AppBridge {
  const { place, ports } = options
  let lifecycle: Lifecycle = firstState
  let view: AppViewState = firstView
  let call = options.call
  let context = options.context
  let resource: UiResource | undefined
  let told: CallTold = nothingTold
  // The host context the app was last given, to send only what changed.
  let given: JsonObject | undefined
  let cancelDeadline: (() => void) | undefined
  // Each request waiting on a port, by id, with what ends it: its deadline's
  // cancel, and the abort of the signal its port was given.
  const pending = new Map<RequestId, () => void>()
  let logged = 0

  const address: AppAddress = {
    sessionId: call.sessionId,
    server: options.server,
    app: {
      executionId: call.executionId,
      toolId: call.toolId,
      instanceId: ports.newId(),
    },
  }
  const gone = () => lifecycle.kind === "gone"
  // Aborted when the mount is released: a read in flight fetches nothing more.
  const mount = new AbortController()
  // Whether this mount asked its server anything: one that never did has
  // nothing to release (M2). A mount is live only once its resource was
  // read here, so one that speaks to its conversation has always asked, and
  // its release withdraws the message it has in review and drops its
  // context (#390, D-G; `bridge.test.ts` D10).
  let asked = false
  // This mount's requests to its server, each at its own address.
  const toServer = {
    readResource(uri: string) {
      asked = true
      return ports.server.readResource(address, uri, mount.signal)
    },
    callTool(tool: string, args: JsonObject) {
      asked = true
      return ports.server.callTool(address, tool, args)
    },
  }

  let cancelReadAgain: (() => void) | undefined
  const initialized = () => lifecycle.kind === "live" || lifecycle.kind === "ending"

  const show = (next: Partial<AppViewState>) => {
    view = { ...view, ...next, lifecycle }
    options.onView(view)
  }

  const send = (message: Outgoing) => {
    if (frameOn(lifecycle)) options.post(message)
  }

  const hostContext = () => appHostContext(place, context, ports.page(), call.definition)

  function hostCapabilities(): JsonObject {
    return {
      serverTools: {},
      serverResources: {},
      logging: {},
      sandbox: { permissions: {}, csp: resource ? approvedDomains(resource.csp) : {} },
      ...(ports.links ? { openLinks: {} } : {}),
      ...(ports.downloads ? { downloadFile: {} } : {}),
      ...(ports.conversation
        ? {
            message: { ...contentModalities },
            updateModelContext: { ...contentModalities, structuredContent: {} },
          }
        : {}),
    }
  }

  function tellCall() {
    if (!initialized()) return
    const { send: messages, told: next } = toolNotifications(told, call.phase)
    told = next
    for (const message of messages) send(message)
  }

  function tellContext() {
    if (!initialized() || !given) return
    const now = hostContext()
    const changed = changedContext(given, now)
    given = now
    if (changed) send(notify("ui/notifications/host-context-changed", changed))
  }

  function run(effect: LifecycleEffect, answering?: RequestId) {
    switch (effect.kind) {
      case "send-document":
        if (resource)
          send(
            notify(sandboxMethods.resourceReady, {
              html: appDocument(resource.html, resource.csp),
              policy: cspPolicy(resource.csp),
              // How long the proxy waits, after each load of the app's frame,
              // for its document to answer (design L32).
              checkWithin: deadlines.initialize,
            }),
          )
        return
      case "answer-initialize":
        given = hostContext()
        if (answering !== undefined)
          send(
            reply(answering, {
              protocolVersion,
              hostInfo: { ...ports.hostInfo },
              hostCapabilities: hostCapabilities(),
              hostContext: given,
            }),
          )
        return
      case "tell-call":
        // What changed while the app was initializing reaches it first.
        tellContext()
        tellCall()
        return
      case "send-teardown":
        send(request(teardownId, "ui/resource-teardown", {}))
        return
      case "close-place":
        options.host.close()
        return
      case "deadline":
        cancelDeadline = ports.timers.after(deadlines[effect.for], () =>
          step({ kind: "deadline" }),
        )
        return
    }
  }

  function step(event: LifecycleEvent, answering?: RequestId) {
    const before = lifecycle
    const next = advance(lifecycle, event)
    if (next.state === before) return
    // Each state's deadline is its own: a change of state clears the last.
    cancelDeadline?.()
    cancelDeadline = undefined
    lifecycle = next.state
    if (lifecycle.kind === "gone") {
      for (const cancel of pending.values()) cancel()
      pending.clear()
      cancelReadAgain?.()
    }
    // The mount ends the first time the view fails or goes: neither state is
    // ever left but for `gone`, and the signal is aborted once, so this is the
    // one release (M2, M3). The reviews it has open are withdrawn with it.
    if (
      (lifecycle.kind === "failed" || lifecycle.kind === "gone") &&
      !mount.signal.aborted
    ) {
      mount.abort()
      if (asked)
        ports.server.release(address).catch((error: unknown) => {
          console.error("An MCP App's release failed", error)
        })
    }
    show({})
    for (const effect of next.effects) run(effect, answering)
  }

  function answerServer(id: RequestId, answer: ServerAnswer) {
    switch (answer.kind) {
      case "ok":
        return send(reply(id, answer.result))
      case "refused":
        return send(refuse(id, errorCodes.refused, answer.reason))
      case "server-gone":
        if (!view.serverGone) show({ serverGone: true })
        return send(refuse(id, errorCodes.refused, "The app's server has stopped"))
      case "busy":
        return send(refuse(id, errorCodes.refused, "Too many requests at once"))
      case "failed":
        return answer.error
          ? send(relayError(id, answer.error))
          : send(refuse(id, errorCodes.internal, "The request failed"))
    }
  }

  /**
   * A request to the conversation that may have been taken all the same
   * (#390, D5–D7): never told as a refusal, which says nothing was, so the
   * app does not send again what may already be the conversation's turn. A
   * server the table places as gone is still shown as gone.
   */
  function answerUncertain(id: RequestId, serverGone: boolean) {
    if (serverGone && !view.serverGone) show({ serverGone: true })
    return send(refuse(id, errorCodes.internal, "The request failed"))
  }

  /**
   * `ui/message`'s answer (#390, D-B): whether the message was taken. What
   * the gateway certainly refused — the person's denial, a turn running, a
   * request it judged — is `isError`, as a tool's refusal is to `tools/call`;
   * nothing was sent for any of them. What may have been taken is a failure
   * (D5–D7). Past the client's bounds, it is the app's request that is wrong
   * (D3).
   */
  function answerMessage(id: RequestId, answer: ConversationAnswer) {
    switch (answer.kind) {
      case "ok":
        return ok(id)
      case "invalid":
        return send(refuse(id, errorCodes.invalidParams, answer.reason))
      case "uncertain":
        return answerUncertain(id, answer.serverGone)
      case "refused":
      case "busy":
        return declined(id)
      case "server-gone":
        if (!view.serverGone) show({ serverGone: true })
        return declined(id)
      case "failed":
        return send(refuse(id, errorCodes.internal, "The request failed"))
    }
  }

  /**
   * `ui/update-model-context`'s answer (#390, D-B): `{}` once taken, and
   * otherwise what a `tools/call` is answered for the same certain outcome,
   * in the same words; what may have been taken is a failure, as for a
   * message (D14); past the client's bounds, the app's request is wrong (D13).
   */
  function answerContext(id: RequestId, answer: ConversationAnswer) {
    switch (answer.kind) {
      case "ok":
        return ok(id)
      case "invalid":
        return send(refuse(id, errorCodes.invalidParams, answer.reason))
      case "uncertain":
        return answerUncertain(id, answer.serverGone)
      default:
        return answerServer(id, answer)
    }
  }

  /**
   * Waits for a port, then answers `id` once: with what the port said, or —
   * past the request deadline — that it timed out, freeing its slot; the
   * port's answer after that is dropped. `send` posts nothing once the view
   * is gone. The port is given a signal aborted once `id` is answered, or the
   * view is gone, or the mount is released, whichever is first: what it has
   * not sent for the request by then, it does not send (D15).
   */
  function settle<T>(
    id: RequestId,
    work: (signal: AbortSignal) => Promise<T>,
    answer: (value: T) => void,
    within: number = deadlines.request,
  ) {
    const request = new AbortController()
    const released = () => request.abort()
    mount.signal.addEventListener("abort", released, { once: true })
    const stopDeadline = ports.timers.after(within, () => {
      if (settled()) send(refuse(id, errorCodes.internal, "The request timed out"))
    })
    function settled() {
      if (!pending.has(id)) return false
      pending.get(id)?.()
      pending.delete(id)
      return true
    }
    pending.set(id, () => {
      stopDeadline()
      mount.signal.removeEventListener("abort", released)
      request.abort()
    })
    work(request.signal)
      .catch((error: unknown) => {
        console.error("An MCP App port failed", error)
        return undefined
      })
      .then((value) => {
        if (!settled()) return
        if (value === undefined)
          send(refuse(id, errorCodes.internal, "The request failed"))
        else answer(value)
      })
  }

  const ok = (id: RequestId) => send(reply(id, {}))
  const declined = (id: RequestId) => send(reply(id, { isError: true }))
  const unsupported = (id: RequestId) =>
    send(refuse(id, errorCodes.methodNotFound, "Not offered by this host"))

  function handle(id: RequestId, message: AppRequest, initialize: Initialize) {
    switch (message.method) {
      case "ui/initialize":
        return send(refuse(id, errorCodes.invalidRequest, "Already initialized"))
      case "ping":
        return ok(id)
      case "tools/call":
        return settle(
          id,
          () => toServer.callTool(message.tool, message.arguments),
          (answer) => answerServer(id, answer),
          ports.server.callWithin,
        )
      case "resources/read":
        return settle(
          id,
          () => toServer.readResource(message.uri),
          (answer) => answerServer(id, answer),
        )
      case "ui/message": {
        const conversation = ports.conversation
        if (!conversation) return unsupported(id)
        return settle(
          id,
          () => conversation.sendMessage(address, message.content),
          (answer) => answerMessage(id, answer),
          conversation.within,
        )
      }
      case "ui/update-model-context": {
        const conversation = ports.conversation
        if (!conversation) return unsupported(id)
        const update = {
          ...(message.content ? { content: message.content } : {}),
          ...(message.structuredContent
            ? { structuredContent: message.structuredContent }
            : {}),
        }
        return settle(
          id,
          (signal) => conversation.updateModelContext(address, update, signal),
          (answer) => answerContext(id, answer),
          conversation.within,
        )
      }
      case "ui/open-link": {
        const links = ports.links
        if (!links) return unsupported(id)
        const url = webUrl(message.url)
        if (url === undefined)
          return send(refuse(id, errorCodes.invalidParams, "Invalid URL"))
        return settle(
          id,
          () => links.open(url),
          (done) => (done === "done" ? ok(id) : declined(id)),
        )
      }
      case "ui/download-file": {
        const downloads = ports.downloads
        if (!downloads) return unsupported(id)
        return settle(
          id,
          () => downloads.download(message.contents),
          (done) => (done === "done" ? ok(id) : declined(id)),
        )
      }
      case "ui/request-display-mode": {
        const decision = requestMode(place, message.mode, initialize.displayModes)
        if (decision.open) options.host.open(decision.open)
        return send(reply(id, { mode: decision.answer }))
      }
    }
  }

  function receiveRequest(id: RequestId, message: AppRequest) {
    if (message.method === "ui/initialize" && lifecycle.kind === "loading")
      return step({ kind: "initialize", initialize: message.initialize }, id)
    if (lifecycle.kind === "live" || lifecycle.kind === "ending") {
      // One id is answered once: a second request under a pending id is not.
      if (pending.has(id))
        return console.warn(`[mcp app ${options.server}] a pending id again`)
      if (pending.size >= pendingLimit)
        return send(refuse(id, errorCodes.refused, "Too many requests"))
      return handle(id, message, lifecycle.initialize)
    }
    if (message.method === "ping") return ok(id)
    if (message.method === "ui/initialize" && lifecycle.kind === "initializing")
      return send(refuse(id, errorCodes.invalidRequest, "Already initialized"))
    return send(refuse(id, errorCodes.notInitialized, "Not initialized"))
  }

  function receive(message: FromFrame) {
    if (gone()) return
    switch (message.kind) {
      case "ignored":
        return
      case "refused":
        return send(refuse(message.id, message.code, message.message))
      case "answer":
        if (message.id === teardownId) step({ kind: "teardown-answered" })
        return
      case "request":
        return receiveRequest(message.id, message.request)
      case "notification": {
        const note = message.notification
        switch (note.method) {
          case sandboxMethods.proxyReady:
            return step({ kind: "proxy-ready" })
          case sandboxMethods.appLeft:
            return step({ kind: "app-left" })
          case "ui/notifications/initialized":
            return step({ kind: "initialized" })
          case "ui/notifications/request-teardown":
            return step({ kind: "request-teardown", place })
          case "ui/notifications/size-changed":
            if (place !== "inline" || !initialized() || note.height === undefined) return
            {
              const height = Math.min(Math.ceil(note.height), inlineMaxHeight)
              // An app may say its size as often as it likes; only a change draws.
              return height === view.height ? undefined : show({ height })
            }
          case sandboxMethods.cspViolation: {
            if (lifecycle.kind === "reading" || lifecycle.kind === "proxy") return
            const { origin } = note
            const known =
              origin === undefined ||
              view.blocked.includes(origin) ||
              view.blocked.length >= blockedOrigins
            // A report that adds nothing draws nothing.
            if (known && view.anyBlocked) return
            return show({
              anyBlocked: true,
              ...(known ? {} : { blocked: [...view.blocked, origin] }),
            })
          }
          case "notifications/message":
            if (logged++ < logLimit)
              console.info(`[mcp app ${options.server}] ${note.level}`)
            return
        }
      }
    }
  }

  options.onView(view)
  // Read the app's resource over its session's connection: once, or again
  // while the gateway answers that it is busy (L1b). A window with no sandbox
  // can show no app, and reads nothing for one.
  if (ports.sandbox) readResource(busyReads)
  else step({ kind: "read", outcome: "unloadable" })
  function readResource(reads: number) {
    toServer
      .readResource(call.resourceUri)
      .catch((error: unknown) => {
        console.error("An MCP App port failed", error)
        return { kind: "failed" } as const
      })
      .then((answer) => {
        if (gone()) return
        if (answer.kind === "busy" && reads > 1) {
          cancelReadAgain = ports.timers.after(deadlines.retryRead, () => {
            cancelReadAgain = undefined
            readResource(reads - 1)
          })
          return
        }
        if (answer.kind === "ok") resource = uiResource(answer.result, call.resourceUri)
        step({
          kind: "read",
          outcome:
            answer.kind === "server-gone"
              ? "server-gone"
              : resource
                ? "html"
                : "unloadable",
        })
      })
  }

  return {
    receive,
    setCall(next) {
      // One view is one call, at one address: a call of another session,
      // identity or resource is not this view's, and is not told to it.
      if (callView(next) !== callView(call))
        return console.warn(`[mcp app ${options.server}] another call for this view`)
      call = next
      tellCall()
    },
    setContext(next) {
      context = next
      tellContext()
    },
    remove() {
      step({ kind: "removed" })
    },
    view: () => view,
  }
}

/** `url` when it is an absolute `http` or `https` URL, as the URL parser spells it. */
function webUrl(url: string): string | undefined {
  try {
    const parsed = new URL(url)
    return parsed.protocol === "http:" || parsed.protocol === "https:"
      ? parsed.href
      : undefined
  } catch {
    return undefined
  }
}
