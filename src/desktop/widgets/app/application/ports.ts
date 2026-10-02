/**
 * What an app view needs from outside the window, in the window's words
 * (ADR 344): its server, the tool call it was made for, the conversation it
 * may speak to, and the person's browser and files. Composition supplies
 * each; tests supply fakes that can give every answer, the failures
 * included. A port that is not supplied is a capability the host does not
 * declare, and its requests are refused — never answered as if done
 * (gate 7).
 *
 * A port call is given a deadline by the bridge (`deadlines.request`): past
 * it the app is answered that it timed out and its slot is freed, so a port
 * that never settles cannot hold the app's requests (`bridge.test.ts`, L31).
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
  | { readonly kind: "failed" }

/** Which session's connection to which server a call goes over. */
export interface ServerAddress {
  readonly sessionId: string
  readonly server: string
}

/** The MCP server an app belongs to, over the gateway's connection for its session (#346, #348). */
export interface McpAppServer {
  /** `resources/read` (`ReadResourceResult`). */
  readResource(address: ServerAddress, uri: string): Promise<ServerAnswer>
  /** `tools/call` (`CallToolResult`). */
  callTool(address: ServerAddress, tool: string, args: JsonObject): Promise<ServerAnswer>
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

/** The conversation an app's call was made in: messages and model context. */
export interface McpAppConversation {
  /** `ui/message`: a message from the person, through the app. */
  sendMessage(sessionId: string, content: readonly JsonObject[]): Promise<Delivered>
  /** `ui/update-model-context`: replaces what this app last gave the model. */
  updateModelContext(
    sessionId: string,
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
  /** Where the proxy is; absent when this window has none, and no app can be shown. */
  readonly sandbox?: SandboxOrigin
  /** What the host calls itself to an app (`hostInfo`). */
  readonly hostInfo: { readonly name: string; readonly version: string }
  /** What the page says of itself now: its style variables, time zone and platform. */
  readonly page: () => PageContext
}
