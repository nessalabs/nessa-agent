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
 * plugin is `missing` (C9). A widget id names its conversation, so two
 * conversations' calls are never one (C10). A view is the whole of its
 * conversation (`ConversationView`, a full replacement), so a call that left
 * it is forgotten (C11): what is kept is bounded by what the views hold.
 */
import type { ConversationTool } from "@nessa/client"
import type { CallRead, McpAppCalls } from "../../application/ports"
import { appWidget } from "../../model/app-ref"
import { isObject, type Json, type JsonObject } from "../../model/json-rpc"
import type { AppCall, CallPhase } from "../../model/tool-call"

/** The calls the window has seen, by server, and what each plugin reads of them. */
export interface GatewayAppCalls {
  /**
   * What a conversation's view now says of its tools: the whole of it. A call
   * that declared a UI is known from here on, under its server; one unchanged
   * since the last view keeps its value, and its readers are not told (C7);
   * one this conversation's last view had and this one does not is forgotten
   * (C11). Returns the servers of the app calls in `tools`, so a plugin can be
   * registered for each.
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
    widgetId: appWidget(mcp.server, conversationId, tool.executionId, tool.toolId).id,
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
  /** One call as it was last read, under its server and widget id. */
  interface Known {
    readonly server: string
    readonly widgetId: string
    /** The call as JSON text, to tell a change from a repeat. */
    readonly from: string
    readonly read: CallRead
  }
  // Each conversation's calls, by the key below: the whole of its last view.
  const conversations = new Map<string, Map<string, Known>>()
  const listeners = new Map<string, Set<() => void>>()
  const keyOf = (server: string, widgetId: string) => JSON.stringify([server, widgetId])
  // Where a server's widget id is read: the conversation is the widget id's
  // own first identity, so each key lives in at most one conversation.
  const index = new Map<string, Known>()

  return {
    observe(conversationId, tools) {
      const before = conversations.get(conversationId) ?? new Map<string, Known>()
      const after = new Map<string, Known>()
      const changed: string[] = []
      for (const tool of tools) {
        const app = gatewayAppCall(conversationId, tool)
        if (!app) continue
        const key = keyOf(app.server, app.widgetId)
        const from = JSON.stringify(app.call)
        const kept = before.get(key)
        if (kept?.from === from) {
          after.set(key, kept)
          continue
        }
        const known: Known = {
          server: app.server,
          widgetId: app.widgetId,
          from,
          read: { kind: "known", call: app.call },
        }
        after.set(key, known)
        index.set(key, known)
        changed.push(key)
      }
      for (const key of before.keys())
        if (!after.has(key)) {
          index.delete(key)
          changed.push(key)
        }
      if (after.size > 0) conversations.set(conversationId, after)
      else conversations.delete(conversationId)
      for (const key of changed)
        for (const listener of [...(listeners.get(key) ?? [])]) listener()
      return [...new Set([...after.values()].map((known) => known.server))]
    },
    forServer(server) {
      return {
        read: (widgetId) => index.get(keyOf(server, widgetId))?.read ?? missing,
        subscribe: (widgetId, listener) => {
          const key = keyOf(server, widgetId)
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
