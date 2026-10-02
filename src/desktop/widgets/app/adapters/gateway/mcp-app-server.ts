/**
 * An app's server, over the gateway (#384): `McpAppServer` on the client's
 * `mcpApps` (#348). Who may call what is the gateway's to decide; this reads
 * its answers into the port's words (`ServerAnswer`), as the design's table
 * says:
 *
 * - a result is `ok`, `isError: true` inside it included — a result for the
 *   app, not a refusal;
 * - a refusal made before anything reached the server, or for want of room,
 *   is `refused`, in words the app is shown;
 * - a conversation with no session of the server any more is `server-gone`;
 * - anything else that did not answer — a timeout, the server's own error, a
 *   result too large, a ticket that was not redeemed or bytes that are not
 *   the ones described, no answer at all — is `failed`.
 *
 * A resource is read in two steps: `readResource` describes it and gives a
 * ticket, and `fetchResource` hands back the bytes only once their size and
 * SHA-256 are the described ones. The bridge is given the `ReadResourceResult`
 * shape it reads (`model/resource.ts`): the HTML, its type, and `_meta.ui`.
 *
 * A port call never rejects for an answer the gateway gave: what rejects is a
 * fault of this adapter or the client's argument checks, which the bridge
 * logs and answers as `failed`.
 */
import {
  ConversationErrorCode,
  MAX_MCP_ARGUMENTS_BYTES,
  mcpAppDeadlines,
  NessaMcpAppError,
  NessaMcpResourceError,
  type McpAppsApi,
  type McpReadResourceResult,
} from "@nessa/client"
import { copyJson, isObject, type Json, type JsonObject } from "../../model/json-rpc"
import type { McpAppServer, ServerAddress, ServerAnswer } from "../../application/ports"

/** What the app is told of a refusal, by the gateway's code: for a tool call, and for a resource. */
const refusals: Readonly<Record<string, { tool: string; resource: string }>> = {
  [ConversationErrorCode.McpApprovalDenied]: {
    tool: "The person declined this action",
    resource: "The person declined this action",
  },
  [ConversationErrorCode.McpApprovalExpired]: {
    tool: "No one answered the request in time",
    resource: "No one answered the request in time",
  },
  [ConversationErrorCode.McpCancelled]: {
    tool: "The request was cancelled",
    resource: "The request was cancelled",
  },
  [ConversationErrorCode.McpToolNotForApp]: {
    tool: "This app may not use that tool",
    resource: "This app may not use that resource",
  },
  [ConversationErrorCode.McpServerMismatch]: {
    tool: "This app may not use that tool",
    resource: "This app may not use that resource",
  },
  [ConversationErrorCode.McpAppUnknown]: {
    tool: "This app may not use that tool",
    resource: "This app may not use that resource",
  },
  [ConversationErrorCode.McpRequestTooLarge]: {
    tool: "The request is too large",
    resource: "The request is too large",
  },
  [ConversationErrorCode.TemporarilyUnavailable]: {
    tool: "Too many requests",
    resource: "Too many requests",
  },
}

const utf8 = new TextEncoder()
const failed: ServerAnswer = Object.freeze({ kind: "failed" })
const serverGone: ServerAnswer = Object.freeze({ kind: "server-gone" })

/** The port's answer for a call that did not answer, or rethrown when it is no answer of the gateway's. */
function unanswered(error: unknown, ask: "tool" | "resource"): ServerAnswer {
  if (error instanceof NessaMcpResourceError) return failed
  if (!(error instanceof NessaMcpAppError)) throw error
  const code = error.code
  if (code === ConversationErrorCode.McpSessionUnavailable) return serverGone
  if (code !== undefined && Object.hasOwn(refusals, code))
    return { kind: "refused", reason: refusals[code][ask] }
  return failed
}

/** The bytes as UTF-8 text, or `undefined` when they are not. */
function text(bytes: Uint8Array): string | undefined {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes)
  } catch {
    return undefined
  }
}

/** The `ReadResourceResult` the bridge reads, from the gateway's description and the checked bytes. */
function readResult(described: McpReadResourceResult, html: string): JsonObject {
  const { csp, permissions } = described
  const ui: Record<string, Json> = {
    csp: {
      connectDomains: [...csp.connectDomains],
      resourceDomains: [...csp.resourceDomains],
      frameDomains: [...csp.frameDomains],
      baseUriDomains: [...csp.baseUriDomains],
    },
    permissions: { ...permissions },
  }
  if (described.domain !== undefined) ui.domain = described.domain
  if (described.prefersBorder !== undefined) ui.prefersBorder = described.prefersBorder
  return {
    contents: [
      { uri: described.uri, mimeType: described.mimeType, text: html, _meta: { ui } },
    ],
  }
}

/** The server's `CallToolResult`, as the client handed it over encoded. */
function toolResult(resultJson: string): ServerAnswer {
  let decoded: unknown
  try {
    decoded = JSON.parse(resultJson)
  } catch {
    return failed
  }
  const result = copyJson(decoded)
  return isObject(result) ? { kind: "ok", result } : failed
}

/** `McpAppServer` over `apps`, the client's MCP App calls. */
export function gatewayMcpAppServer(apps: McpAppsApi): McpAppServer {
  return {
    async callTool(address: ServerAddress, tool: string, args: JsonObject) {
      const argumentsJson = JSON.stringify(args)
      // The most a review shows: refused here, as the gateway would, rather
      // than the client's own check answering the app.
      if (utf8.encode(argumentsJson).byteLength > MAX_MCP_ARGUMENTS_BYTES)
        return {
          kind: "refused",
          reason: refusals[ConversationErrorCode.McpRequestTooLarge].tool,
        }
      try {
        const { resultJson } = await apps.callTool(
          address.sessionId,
          address.app,
          address.server,
          tool,
          argumentsJson,
        )
        return toolResult(resultJson)
      } catch (error) {
        return unanswered(error, "tool")
      }
    },
    async readResource(address: ServerAddress, uri: string) {
      try {
        const described = await apps.readResource(
          address.sessionId,
          address.app,
          address.server,
          uri,
        )
        const bytes = await apps.fetchResource(described.ticket, described)
        const html = text(bytes)
        return html === undefined
          ? failed
          : { kind: "ok", result: readResult(described, html) }
      } catch (error) {
        return unanswered(error, "resource")
      }
    },
    release(address: ServerAddress) {
      apps.releaseApp(address.sessionId, address.app).catch((error: unknown) => {
        console.warn("An MCP App's release was not acknowledged", error)
      })
    },
    // The client's own deadlines: the bridge never gives up on an answer the
    // gateway is still bound to send, a person deciding on a review included.
    answersWithin: Object.freeze({
      callTool: mcpAppDeadlines.callToolMs,
      readResource: mcpAppDeadlines.readResourceMs + mcpAppDeadlines.fetchResourceMs,
    }),
  }
}
