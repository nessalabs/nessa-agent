/**
 * The tool calls an app's widgets name, read from a gateway conversation's
 * view (`ConversationTool`): the `McpAppCalls` of each server's app plugin,
 * from the transcript's real MCP tool calls (the state table on #384, rows
 * C1–C11). The transcript names a call's widget with the same identities
 * (`workspace/adapters/gateway/tool-widget.ts`, `appWidget`), so the widget a
 * message draws and the call it reads here are one fact.
 *
 * What the view says of a call is mapped, not inferred:
 * - its status: `completed` is a result, `failed` an error result, anything
 *   else still running;
 * - its arguments: `mcp.argumentsJson` when the gateway's connection carried
 *   them, and otherwise `input` when that is a JSON object. A running call
 *   whose arguments are not yet known carries none, so `tool-input` waits;
 *   a finished call whose view never carried any is told `{}`, and the
 *   result follows;
 * - its result: the text in `details`, and `structuredContent` when it is a
 *   JSON object.
 *
 * Each server's calls are its own: a widget id read through another server's
 * plugin is `missing` (C9). A widget id names its conversation, so two
 * conversations' calls are never one (C10).
 *
 * A view is bounded — the gateway keeps its latest tools, and drops the oldest
 * — so a call missing from a later view has not ended: it keeps the last state
 * a view reported, and its app stays (C11, `app-calls.test.ts`). A
 * conversation's calls go only with the conversation (`forget`, which the
 * gateway source calls when a read says it was deleted); a forgotten
 * conversation is not brought back by a view of it arriving late. The order
 * of a conversation's views is its source's to keep (`gateway-source.ts`
 * tells them in the order read): `ConversationView.revision` is for
 * equality, not order.
 */
import type { ConversationTool, ConversationView } from "@nessa/client"
import type { CallRead, McpAppCalls } from "../../application/ports"
import { appWidget } from "../../model/app-ref"
import { isObject, type Json, type JsonObject } from "../../model/json-rpc"
import type { AppCall, CallPhase } from "../../model/tool-call"

/** The calls the window has seen, by server, and what each plugin reads of them. */
export interface GatewayAppCalls {
  /**
   * What a conversation's view now says of its tools. A call that declared a
   * UI is known from here on, under its server; one unchanged since the last
   * view keeps its value, and its readers are not told (C7); one the view no
   * longer reports keeps its last state (C11). Returns the servers of the app
   * calls in `tools`, so a plugin can be registered for each.
   */
  observe(view: AppCallsView): readonly string[]
  /**
   * The conversation was deleted: its calls are forgotten, their readers told,
   * and any later view of it is ignored (C11). Only for a deletion: a deleted
   * conversation's id is never used again, while a closed one reopens under
   * the same id.
   */
  forget(conversationId: string): void
  /** The calls port of `server`'s app plugin. */
  forServer(server: string): McpAppCalls
}

/** What of a conversation's view the calls are read from: its own id, and its tools. */
export type AppCallsView = Pick<ConversationView, "conversationId" | "tools">

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

/**
 * The arguments the app is told, when they are known. The connection's
 * encoding wins over a permission review's `input`. Either missing, or not
 * one JSON object, is not yet known.
 */
function argumentsOf(tool: ConversationTool): JsonObject | undefined {
  const carried = tool.mcp?.argumentsJson
  if (carried !== undefined) return jsonObject(carried)
  return jsonObject(tool.input)
}

function phaseOf(tool: ConversationTool): CallPhase {
  const args = argumentsOf(tool)
  if (tool.status !== "completed" && tool.status !== "failed")
    return args === undefined ? { kind: "running" } : { kind: "running", arguments: args }
  const structuredContent = jsonObject(tool.structuredContent)
  const result: JsonObject = {
    content: tool.details ? [{ type: "text", text: tool.details }] : [],
    ...(structuredContent ? { structuredContent } : {}),
    ...(tool.status === "failed" ? { isError: true } : {}),
  }
  return { kind: "done", arguments: args ?? {}, result }
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
  /** One call as it was last read. */
  interface Known {
    readonly call: AppCall
    readonly read: CallRead
  }
  // Each conversation's calls, by server and widget id (`keyOf`).
  const conversations = new Map<string, Map<string, Known>>()
  // Conversations deleted. Their ids are never used again (the protocol's
  // `conversation_deleted`), so a view of one arriving late is ignored rather
  // than bringing its calls back: kept as ids alone.
  const forgotten = new Set<string>()
  const listeners = new Map<string, Set<() => void>>()
  const keyOf = (server: string, widgetId: string) => JSON.stringify([server, widgetId])
  // Where a server's widget id is read. The conversation is the widget id's
  // own first identity, so each key lives in at most one conversation.
  const index = new Map<string, Known>()
  const tell = (keys: readonly string[]) => {
    for (const key of keys)
      for (const listener of [...(listeners.get(key) ?? [])]) listener()
  }

  return {
    observe({ conversationId, tools }) {
      if (forgotten.has(conversationId)) return []
      let calls = conversations.get(conversationId)
      const servers = new Set<string>()
      const changed: string[] = []
      for (const tool of tools) {
        const app = gatewayAppCall(conversationId, tool)
        if (!app) continue
        servers.add(app.server)
        const key = keyOf(app.server, app.widgetId)
        const before = calls?.get(key)?.call
        // A repeat changes nothing, and tells no one (C7).
        if (before && JSON.stringify(before) === JSON.stringify(app.call)) continue
        if (!calls) conversations.set(conversationId, (calls = new Map()))
        const known: Known = { call: app.call, read: { kind: "known", call: app.call } }
        calls.set(key, known)
        index.set(key, known)
        changed.push(key)
      }
      tell(changed)
      return [...servers]
    },
    forget(conversationId) {
      forgotten.add(conversationId)
      const calls = conversations.get(conversationId)
      if (!calls) return
      conversations.delete(conversationId)
      for (const key of calls.keys()) index.delete(key)
      tell([...calls.keys()])
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
