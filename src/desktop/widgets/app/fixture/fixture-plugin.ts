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
 * gives it one, is the sample workspace's (`fixtureConversation`, #390): a
 * message within the client's bounds lands there written by the app, or is
 * refused while the sample's agent is at work; a context is refused, as the
 * sample has no model to give it to.
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
import { appMessageText } from "../application/app-message"
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

/** What the fixture's conversation answers a context: the sample has no model (gate 7). */
export const noModelForContext = "The sample has no model to give context to"

/**
 * The fixture app's conversation: its message given to `write` as the
 * fixture server's app, its blocks' text as the gateway's adapter makes it
 * and held to the same bounds (`appMessageText`, `application/app-message.ts`):
 * past them it is `invalid`, in the client's words, and nothing is written —
 * so the sample draws no message a gateway would refuse (gate 7). A message `write` refuses is the
 * app's message refused (`isError`), and nothing was written. The sample's
 * replies are scripted, so a context is refused, never answered as if a
 * model had it.
 */
export function fixtureConversation(
  write: (
    sessionId: string,
    app: { readonly server: string; readonly tool: string },
    text: string,
  ) => Promise<void>,
): McpAppConversation {
  return {
    within: deadlines.request,
    sendMessage: async (address, content) => {
      const message = appMessageText(content)
      if (message.kind === "invalid") return message
      return write(
        address.sessionId,
        { server: address.server, tool: fixtureCall(address.sessionId).tool },
        message.text,
      ).then(
        () => ({ kind: "ok", result: {} }) as const,
        () =>
          ({ kind: "refused", reason: "The sample did not take the message" }) as const,
      )
    },
    updateModelContext: async () => ({ kind: "refused", reason: noModelForContext }),
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
  /** Its conversation; without one, `ui/message` and its context are not offered. */
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
