/**
 * What an app view needs from outside the window, in the window's words
 * (ADR 344): its server, the tool call it was made for, the conversation it
 * may speak to, and the person's browser and files. Composition supplies
 * each; tests supply fakes that can give every answer, the failures
 * included. A port that is not supplied is a capability the host does not
 * declare, and its requests are refused — never answered as if done
 * (gate 7).
 *
 * A port call is given a deadline by the bridge (`deadlines.request`, or the
 * server's own `callWithin` for `tools/call`): past it the app is answered
 * that it timed out and its slot is freed, so a port that never settles
 * cannot hold the app's requests (`bridge.test.ts`, L31).
 * A port that rejects is a fault of its adapter, not an answer: the bridge
 * logs it and answers the app as `failed`.
 */
import type { PageContext } from "../model/host-context"
import type { DownloadFile } from "../model/messages"
import type { JsonObject } from "../model/json-rpc"
import type { AppCall } from "../model/tool-call"

/**
 * What the server's side said to an app's call. Who may call what — a tool
 * the app may not see, another server's tool, an approval the person
 * refused — is the gateway's to decide (#348); the window only carries its
 * answer.
 */
export type ServerAnswer =
  | { readonly kind: "ok"; readonly result: JsonObject }
  /** Not allowed, with the gateway's reason in words the app is shown. */
  | { readonly kind: "refused"; readonly reason: string }
  /** The server, or its session, is gone: nothing more will reach it. */
  | { readonly kind: "server-gone" }
  /**
   * The gateway has no room for another app call just now, and nothing
   * reached the server: the same request may be made again.
   */
  | { readonly kind: "busy" }
  /**
   * It did not answer. `error` is the server's own JSON-RPC error, when it
   * sent one: passed to the app as it came, its code a signed integer.
   */
  | { readonly kind: "failed"; readonly error?: ServerError }

/** A JSON-RPC error the app's server answered with. */
export interface ServerError {
  readonly code: number
  readonly message: string
}

/**
 * Who is asking, and over what: the session (the gateway's conversation) the
 * app's tool call was made in, the server it belongs to, and the app itself —
 * its tool call, by the execution and tool ids that name it, and this mount of
 * it. The bridge makes it from the view's own call and never from anything the
 * app says (`bridge.test.ts`, "forged identity").
 */
export interface AppAddress {
  readonly sessionId: string
  readonly server: string
  readonly app: {
    readonly executionId: string
    readonly toolId: string
    /** One per mount of the app (#349's view); minted by the bridge. */
    readonly instanceId: string
  }
}

/** The MCP server an app belongs to, over the gateway's connection for its conversation (#346, #348). */
export interface McpAppServer {
  /**
   * `resources/read` (`ReadResourceResult`). `signal` is aborted when the
   * mount is released: what the read has not fetched yet, it does not fetch.
   */
  readResource(
    address: AppAddress,
    uri: string,
    signal: AbortSignal,
  ): Promise<ServerAnswer>
  /** `tools/call` (`CallToolResult`). */
  callTool(address: AppAddress, tool: string, args: JsonObject): Promise<ServerAnswer>
  /**
   * This mount is torn down: the reviews it has open are withdrawn and its
   * resource tickets released. Sent once, when the view first fails or ends
   * (#384, M2).
   */
  release(address: AppAddress): Promise<void>
  /**
   * How long, in milliseconds, a `tools/call` may take before the bridge gives
   * up on it: a destructive tool waits on the person's review first.
   */
  readonly callWithin: number
}

/** What the conversation holds of the call a widget names. */
export type CallRead =
  | { readonly kind: "unread" }
  | { readonly kind: "missing" }
  | { readonly kind: "known"; readonly call: AppCall }

/** The tool calls an app's widgets name, by widget id, read as they change. */
export interface McpAppCalls {
  /** The call now; the same value while nothing about it changed. */
  read(widgetId: string): CallRead
  /** Tells `listener` of each change to the call; returns its stop. */
  subscribe(widgetId: string, listener: () => void): () => void
}

/** What a request to the conversation came to. */
export type Delivered = "done" | "refused"

/**
 * The conversation an app's call was made in: messages and model context.
 * Whether the app may speak there — the person's consent, a turn already
 * running, the bounds — is the gateway's to decide (#390); `refused` is its
 * answer.
 */
export interface McpAppConversation {
  /**
   * `ui/message`: a message from the person, written by the app. Its blocks
   * are what `model/messages.ts` reads: text.
   */
  sendMessage(address: AppAddress, content: readonly JsonObject[]): Promise<Delivered>
  /**
   * How long, in milliseconds, either request may take before the bridge
   * gives up on it: the first of a mount's messages waits on the person's
   * review, and either may wait for the conversation to open.
   */
  readonly within: number
  /** `ui/update-model-context`: replaces what this mount last gave the model. */
  updateModelContext(
    address: AppAddress,
    context: {
      readonly content?: readonly JsonObject[]
      readonly structuredContent?: JsonObject
    },
  ): Promise<Delivered>
}

/** Opens an `http`/`https` URL in the person's browser (`ui/open-link`). */
export interface LinkOpener {
  open(url: string): Promise<Delivered>
}

/** Saves files the app hands over (`ui/download-file`). */
export interface FileDownloader {
  download(files: readonly DownloadFile[]): Promise<Delivered>
}

/** The clock's timers, for the bridge's deadlines. */
export interface Timers {
  /** Runs `run` after `ms`; returns its cancel. */
  after(ms: number, run: () => void): () => void
}

/** Where the sandbox proxy is served: its page, and the origin its messages carry. */
export interface SandboxOrigin {
  readonly url: string
  readonly origin: string
}

/** Everything an app view reaches outside the window. */
export interface McpAppPorts {
  readonly server: McpAppServer
  readonly calls: McpAppCalls
  readonly conversation?: McpAppConversation
  readonly links?: LinkOpener
  readonly downloads?: FileDownloader
  readonly timers: Timers
  /** A fresh lowercase UUID, for each mount's `instanceId` (the protocol's form). */
  readonly newId: () => string
  /** Where the proxy is; absent when this window has none, and no app can be shown. */
  readonly sandbox?: SandboxOrigin
  /** What the host calls itself to an app (`hostInfo`). */
  readonly hostInfo: { readonly name: string; readonly version: string }
  /** What the page says of itself now: its style variables, time zone and platform. */
  readonly page: () => PageContext
}
