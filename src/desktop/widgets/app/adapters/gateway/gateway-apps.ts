/**
 * Real servers' apps, through the gateway (#384): for each MCP server a
 * conversation's view names with a UI, one app plugin (`mcp:` its name) is
 * registered while the window runs, its server port over `client.mcpApps`
 * (`mcp-app-server.ts`) and its calls read from the views (`app-calls.ts`).
 * The workspace's gateway source (#248) hands each view's tools to `observe`,
 * as it makes the transcript's widget parts from them, and a deleted
 * conversation to `forget`.
 */
import type { McpAppsApi } from "@nessa/client"
import type { DesktopWidgetRegistry } from "../../../ui/plugin"
import type { McpAppPorts } from "../../application/ports"
import { appPluginId } from "../../model/app-ref"
import { appPlugin } from "../../ui/app-plugin"
import { gatewayAppCalls, type AppCallsView } from "./app-calls"
import { gatewayAppServer } from "./mcp-app-server"

/** What every server's app shares: everything in its ports but its calls and its server. */
export type SharedAppPorts = Omit<McpAppPorts, "calls" | "server">

export interface GatewayApps {
  /** What a conversation's view now says of its tools (`GatewayAppCalls.observe`). */
  observe(view: AppCallsView): void
  /** The conversation was deleted: its apps' calls with it (`GatewayAppCalls.forget`). */
  forget(conversationId: string): void
}

export function gatewayApps(options: {
  readonly registry: DesktopWidgetRegistry
  readonly mcpApps: McpAppsApi
  readonly ports: SharedAppPorts
}): GatewayApps {
  const calls = gatewayAppCalls()
  const server = gatewayAppServer(options.mcpApps, options.ports.timers.after)
  return {
    observe(view) {
      for (const name of calls.observe(view)) {
        // A server's app is registered once (C8); a plugin already under its
        // id — this one, or composition's — is left as it is.
        if (options.registry.plugin(appPluginId(name))) continue
        options.registry.register(
          appPlugin({
            server: name,
            name,
            ports: { ...options.ports, server, calls: calls.forServer(name) },
          }),
        )
      }
    },
    forget: (conversationId) => calls.forget(conversationId),
  }
}
