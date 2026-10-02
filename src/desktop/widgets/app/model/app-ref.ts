/**
 * How an MCP App's widgets are named (ADR 326's `WidgetRef`, ADR 344): the
 * plugin is the server's app, `mcp:` then the server's name; the widget is
 * one tool call, by the execution and tool ids that name it. The one
 * statement of both: the transcript names a call's widget with them
 * (`workspace/model/transcript.ts`, `toolWidget`), and `appPlugin` derives
 * an app plugin's id from its server with `appPluginId`
 * (`app-ref.test.ts`). A plugin built by hand is not held to it.
 */
import type { WidgetRef } from "../../model/widget-ref"

/** The plugin id a server's app is registered under. */
export function appPluginId(server: string): string {
  return `mcp:${server}`
}

/**
 * The widget a tool call of `server`'s is drawn as. The id is the two
 * identities as a JSON array, so no spelling of either can make two calls
 * one widget.
 */
export function appWidget(
  server: string,
  executionId: string,
  toolId: string,
): WidgetRef {
  return { plugin: appPluginId(server), id: JSON.stringify([executionId, toolId]) }
}
