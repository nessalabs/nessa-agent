/**
 * The tool calls an app's widgets name, read from a gateway conversation's
 * view (`ConversationTool`): the `McpAppCalls` of each server's app plugin,
 * from the transcript's real MCP tool calls (the state table on #384, rows
 * C1–C9). The transcript names a call's widget with the same identities
 * (`workspace/adapters/gateway/tool-widget.ts`, `appWidget`), so the widget a
 * message draws and the call it reads here are one fact.
 *
 * What the view says of a call is mapped, not inferred:
 * - its status: `completed` is a result, `failed` an error result, anything
 *   else still running;
 * - its arguments: `input`, when it is a JSON object. The view holds them only
 *   when a permission request showed them; otherwise the app is told none
 *   (`{}`), the limit #394 removes;
 * - its result: the text in `details`, and `structuredContent` when it is a
 *   JSON object.
 *
 * Each server's calls are its own: a widget id read through another server's
 * plugin is `missing` (C9).
 */
import type { ConversationTool } from "@nessa/client"
import type { CallRead, McpAppCalls } from "../../application/ports"
import { appWidget } from "../../model/app-ref"
import { isObject, type Json, type JsonObject } from "../../model/json-rpc"
import type { AppCall, CallPhase } from "../../model/tool-call"

/** The calls the window has seen, by server, and what each plugin reads of them. */
export interface GatewayAppCalls {
  /**
   * What a conversation's view now says of its tools. A call that declared a
   * UI is known from here on, under its server; one unchanged since the last
   * view keeps its value, and its readers are not told (C7). Returns the
   * servers of the app calls in `tools`, so a plugin can be registered for each.
   */
  observe(conversationId: string, tools: readonly ConversationTool[]): readonly string[]
  /** The calls port of `server`'s app plugin. */
  forServer(server: string): McpAppCalls
}

const missing: CallRead = { kind: "missing" }

/** A JSON object read from `text`, or `undefined` for anything else. */
function jsonObject(text: string | undefined): JsonObject | undefined {
  if (!text) return undefined
  try {
    const value = JSON.parse(text) as Json
    return isObject(value) ? value : undefined
  } catch {
    return undefined
  }
}

function phaseOf(tool: ConversationTool): CallPhase {
  const args = jsonObject(tool.input) ?? {}
  if (tool.status !== "completed" && tool.status !== "failed")
    return { kind: "running", arguments: args }
  const structuredContent = jsonObject(tool.structuredContent)
  const result: JsonObject = {
    content: tool.details ? [{ type: "text", text: tool.details }] : [],
    ...(structuredContent ? { structuredContent } : {}),
    ...(tool.status === "failed" ? { isError: true } : {}),
  }
  return { kind: "done", arguments: args, result }
}

/** The app call `tool` is, in conversation `conversationId`, or `null` for a call with no UI (C1, C2). */
export function gatewayAppCall(
  conversationId: string,
  tool: ConversationTool,
): { readonly server: string; readonly widgetId: string; readonly call: AppCall } | null {
  const mcp = tool.mcp
  if (!mcp?.resourceUri) return null
  return {
    server: mcp.server,
    widgetId: appWidget(mcp.server, tool.executionId, tool.toolId).id,
    call: {
      sessionId: conversationId,
      executionId: tool.executionId,
      toolId: tool.toolId,
      tool: mcp.tool,
      resourceUri: mcp.resourceUri,
      phase: phaseOf(tool),
    },
  }
}

export function gatewayAppCalls(): GatewayAppCalls {
  // Per server, per widget id: what was read, and the text it was read from.
  const known = new Map<string, Map<string, { from: string; read: CallRead }>>()
  const listeners = new Map<string, Set<() => void>>()
  const listenerKey = (server: string, widgetId: string) =>
    JSON.stringify([server, widgetId])

  return {
    observe(conversationId, tools) {
      const servers = new Set<string>()
      const changed: string[] = []
      for (const tool of tools) {
        const app = gatewayAppCall(conversationId, tool)
        if (!app) continue
        servers.add(app.server)
        let calls = known.get(app.server)
        if (!calls) known.set(app.server, (calls = new Map()))
        const from = JSON.stringify([conversationId, app.call])
        if (calls.get(app.widgetId)?.from === from) continue
        calls.set(app.widgetId, { from, read: { kind: "known", call: app.call } })
        changed.push(listenerKey(app.server, app.widgetId))
      }
      for (const key of changed)
        for (const listener of [...(listeners.get(key) ?? [])]) listener()
      return [...servers]
    },
    forServer(server) {
      return {
        read: (widgetId) => known.get(server)?.get(widgetId)?.read ?? missing,
        subscribe: (widgetId, listener) => {
          const key = listenerKey(server, widgetId)
          let set = listeners.get(key)
          if (!set) listeners.set(key, (set = new Set()))
          set.add(listener)
          return () => {
            set.delete(listener)
            if (set.size === 0) listeners.delete(key)
          }
        },
      }
    },
  }
}
