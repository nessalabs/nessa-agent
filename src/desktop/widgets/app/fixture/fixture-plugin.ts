/**
 * A fixture MCP server's app, with every port in memory: what composition
 * registers beside the sample workspace, so an app can be seen and measured
 * in a real browser (`verification/desktop/scripts/mcp-apps.mjs`) before the
 * gateway's `mcp.readResource` and `mcp.callTool` (#348) are wired.
 *
 * It stands in for a server and for the gateway in front of it, and decides
 * nothing for either: `fixture_refresh` answers, `fixture_secret` is refused
 * as the gateway refuses a tool hidden from apps, and any other tool is
 * refused too. Its one call is finished, with arguments and a result.
 */
import type { JsonObject } from "../model/json-rpc"
import type { AppCall } from "../model/tool-call"
import type { AppWidgetPlugin } from "../../ui/plugin"
import type {
  CallRead,
  McpAppPorts,
  McpAppServer,
  SandboxOrigin,
  Timers,
} from "../application/ports"
import type { PageContext } from "../model/host-context"
import { appMimeType } from "../model/resource"
import { appPlugin } from "../ui/app-plugin"
import { fixtureAppHtml } from "./fixture-app"
import { fixtureResourceUri, fixtureServer, fixtureWidget } from "./fixture-widgets"

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
  readonly page: () => PageContext
}): AppWidgetPlugin {
  const known: CallRead = { kind: "known", call: fixtureCall(options.sessionId) }
  const missing: CallRead = { kind: "missing" }
  const ports: McpAppPorts = {
    server: fixtureServerPort(),
    calls: {
      read: (id) => (id === fixtureWidget.id ? known : missing),
      subscribe: () => () => {},
    },
    timers: options.timers,
    ...(options.sandbox ? { sandbox: options.sandbox } : {}),
    hostInfo: { name: "Nessa", version: "fixture" },
    page: options.page,
  }
  return appPlugin({ server: fixtureServer, name: "Fixture", ports })
}
