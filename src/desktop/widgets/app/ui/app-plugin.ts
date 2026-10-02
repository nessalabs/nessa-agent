/**
 * An MCP server's app as a widget plugin (`AppWidgetPlugin`): its id is its
 * server's app id (`model/app-ref.ts`), derived here, so the plugin a widget
 * is looked up by and the server its calls go to are one fact.
 */
import type { AppWidgetPlugin } from "../../ui/plugin"
import type { McpAppPorts } from "../application/ports"
import { appPluginId } from "../model/app-ref"

export function appPlugin(options: {
  readonly server: string
  readonly name: string
  readonly ports: McpAppPorts
}): AppWidgetPlugin {
  return {
    kind: "app",
    id: appPluginId(options.server),
    name: options.name,
    server: options.server,
    ports: options.ports,
  }
}
