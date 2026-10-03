/**
 * A fixture MCP server's app, with every port in memory: what composition
 * registers beside the sample workspace, so an app can be seen and measured
 * in a real browser (`verification/desktop/scripts/mcp-apps.mjs`). A real
 * server's app goes through the gateway (`adapters/gateway/`, #384); the
 * fixture stays for the sample workspace and the browser checks.
 *
 * It stands in for a server and for the gateway in front of it, and decides
 * nothing for either: `fixture_refresh` answers, `fixture_secret` is refused
 * as the gateway refuses a tool hidden from apps, and any other tool is
 * refused too. Its one call is finished, with arguments and a result. A
 * release holds nothing to let go of. Its conversation, where composition
 * gives it one, is the sample workspace's (`fixtureConversation`): a message
 * lands there written by the app, and a context is taken and kept nowhere,
 * as the sample's agent reads none.
 */
import type { JsonObject } from "../model/json-rpc"
import type { AppCall } from "../model/tool-call"
import type { AppWidgetPlugin } from "../../ui/plugin"
import type {
  CallRead,
  McpAppConversation,
  McpAppPorts,
  McpAppServer,
  SandboxOrigin,
  Timers,
} from "../application/ports"
import type { PageContext } from "../model/host-context"
import { deadlines } from "../application/bridge"
import { appMimeType } from "../model/resource"
import { appPlugin } from "../ui/app-plugin"
import { fixtureAppHtml } from "./fixture-app"
import {
  fixtureCallIdentity,
  fixtureResourceUri,
  fixtureServer,
  fixtureWidget,
} from "./fixture-widgets"

/** The tools it answers an app for, and the one it refuses as hidden. */
export const fixtureTools = {
  allowed: "fixture_refresh",
  hidden: "fixture_secret",
} as const

/** The fixture server, behind the port the gateway will implement. */
export function fixtureServerPort(html = fixtureAppHtml): McpAppServer {
  return {
    readResource: async (_address, uri) =>
      uri === fixtureResourceUri
        ? {
            kind: "ok",
            result: {
              contents: [{ uri, mimeType: appMimeType, text: html, _meta: { ui: {} } }],
            },
          }
        : { kind: "failed" },
    callTool: async (_address, tool, args) => {
      if (tool === fixtureTools.allowed)
        return {
          kind: "ok",
          result: {
            content: [{ type: "text", text: "Refreshed" }],
            structuredContent: { refreshed: args },
          },
        }
      return { kind: "refused", reason: `${tool} is not available to apps` }
    },
    release: async () => {},
    callWithin: deadlines.request,
  }
}

/**
 * The fixture app's conversation: its message given to `write`, as the
 * fixture server's app, which a refusal of `write` is the app's answer to.
 */
export function fixtureConversation(
  write: (
    sessionId: string,
    app: { readonly server: string; readonly tool: string },
    text: string,
  ) => Promise<void>,
): McpAppConversation {
  return {
    messageWithin: deadlines.request,
    sendMessage: (address, content) =>
      write(
        address.sessionId,
        { server: address.server, tool: fixtureCall(address.sessionId).tool },
        content
          .map((block) => (typeof block.text === "string" ? block.text : ""))
          .join("\n\n"),
      ).then(
        () => "done" as const,
        () => "refused" as const,
      ),
    updateModelContext: async () => "done",
  }
}

/** The fixture's one call, finished, in session `sessionId`. */
export function fixtureCall(sessionId: string): AppCall {
  const result: JsonObject = {
    content: [{ type: "text", text: "Three rows" }],
    structuredContent: { rows: 3 },
  }
  return {
    sessionId,
    ...fixtureCallIdentity,
    tool: "show_fixture",
    definition: { name: "show_fixture", inputSchema: { type: "object" } },
    resourceUri: fixtureResourceUri,
    phase: { kind: "done", arguments: { title: "Fixture" }, result },
  }
}

/** The fixture server's app, its call belonging to `sessionId`. */
export function fixtureAppPlugin(options: {
  readonly sessionId: string
  readonly sandbox: SandboxOrigin | undefined
  readonly timers: Timers
  readonly newId: () => string
  readonly page: () => PageContext
  /** Its conversation: none, and `ui/message` is not offered. */
  readonly conversation?: McpAppConversation
}): AppWidgetPlugin {
  const known: CallRead = { kind: "known", call: fixtureCall(options.sessionId) }
  const missing: CallRead = { kind: "missing" }
  const ports: McpAppPorts = {
    server: fixtureServerPort(),
    calls: {
      read: (id) => (id === fixtureWidget(options.sessionId).id ? known : missing),
      subscribe: () => () => {},
    },
    timers: options.timers,
    newId: options.newId,
    ...(options.sandbox ? { sandbox: options.sandbox } : {}),
    hostInfo: { name: "Nessa", version: "fixture" },
    page: options.page,
    ...(options.conversation ? { conversation: options.conversation } : {}),
  }
  return appPlugin({ server: fixtureServer, name: "Fixture", ports })
}
