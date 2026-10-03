/**
 * How an MCP App's widgets are named (ADR 326's `WidgetRef`, ADR 344): the
 * plugin is the server's app, `mcp:` then the server's name; the widget is
 * one tool call, by the session (conversation) it was made in and the
 * execution and tool ids that name it there. The one
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
 * The widget a tool call of `server`'s is drawn as. The id is the three
 * identities as a JSON array, so no spelling of any can make two calls one
 * widget. The session is among them because an execution id is the caller's
 * to choose and a tool id is unique only within its execution: the same pair
 * in two conversations is two calls (#384, C10).
 */
export function appWidget(
  server: string,
  sessionId: string,
  executionId: string,
  toolId: string,
): WidgetRef {
  return {
    plugin: appPluginId(server),
    id: JSON.stringify([sessionId, executionId, toolId]),
  }
}
