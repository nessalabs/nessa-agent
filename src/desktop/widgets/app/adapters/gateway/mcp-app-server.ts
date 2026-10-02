/**
 * `McpAppServer` over the gateway (`client.mcpApps`, #348(b)): an app's
 * `tools/call` and `resources/read` go to its own server as the gateway's
 * `mcp.callTool` and `mcp.readResource`, and its mount is let go with
 * `mcp.releaseApp`. Who may call what — a tool hidden from apps, another
 * server's, a destructive tool the person must review — is the gateway's to
 * decide; this only says what it answered, in the port's typed outcomes (the
 * state table on #384, rows A1–A12, R1–R5).
 *
 * A resource's bytes are fetched with the single-use ticket `readResource`
 * answers, at once and once. The client's `fetchResource` holds them to the
 * described size and SHA-256 — the one place that is checked — and the ticket
 * goes no further than that call: nothing here keeps, logs or returns it
 * (`mcp-app-server.test.ts`, "the ticket").
 *
 * Arguments past the client's published bound (`MAX_MCP_ARGUMENTS_BYTES`) are
 * refused before anything is sent (A2): the one bound the app controls that a
 * review is held to. Any other `TypeError` the client throws — an address the
 * host built, a name past its bound — is a fault, logged, never told to the
 * app as its own refusal. A read whose mount was released fetches nothing more
 * (R6).
 *
 * `callTool` and `readResource` settle with an outcome whatever the client
 * throws, so the bridge never mistakes one for a fault of this adapter
 * (`mcp-app-server.test.ts`, "never rejects"). Only `release` rejects, when
 * the gateway did not take it.
 */
import {
  ConversationErrorCode,
  MAX_MCP_ARGUMENTS_BYTES,
  MCP_APP_CALL_DEADLINE_MS,
  NessaMcpAppError,
  NessaMcpResourceError,
  type McpAppsApi,
  type McpReadResourceResult,
} from "@nessa/client"
import type { AppAddress, McpAppServer, ServerAnswer } from "../../application/ports"
import type { JsonObject } from "../../model/json-rpc"

/** What the gateway refused, in words the app is shown. */
type Refusal = "declined" | "expired" | "withdrawn" | "not-for-app" | "too-large"

/** What one of the gateway's codes comes to (gate 11: every code is placed). */
type Outcome = { readonly refused: Refusal } | "busy" | "server-gone" | "failed"

const outcomes: Record<ConversationErrorCode, Outcome> = {
  [ConversationErrorCode.McpApprovalDenied]: { refused: "declined" },
  [ConversationErrorCode.McpApprovalExpired]: { refused: "expired" },
  [ConversationErrorCode.McpCancelled]: { refused: "withdrawn" },
  [ConversationErrorCode.McpToolNotForApp]: { refused: "not-for-app" },
  [ConversationErrorCode.McpServerMismatch]: { refused: "not-for-app" },
  [ConversationErrorCode.McpAppUnknown]: { refused: "not-for-app" },
  [ConversationErrorCode.McpRequestTooLarge]: { refused: "too-large" },
  // No room on the app lane; nothing reached the server (L1b, A8).
  [ConversationErrorCode.TemporarilyUnavailable]: "busy",
  // The server's session, or the conversation it belongs to, is gone.
  [ConversationErrorCode.McpSessionUnavailable]: "server-gone",
  [ConversationErrorCode.ConversationNotFound]: "server-gone",
  [ConversationErrorCode.ConversationDeleted]: "server-gone",
  [ConversationErrorCode.ConversationClosed]: "server-gone",
  // The server was asked and did not answer with a result; `mcp_remote_error`
  // passes on its own error when it sent one (`answerFor`).
  [ConversationErrorCode.McpTimedOut]: "failed",
  [ConversationErrorCode.McpRemoteError]: "failed",
  [ConversationErrorCode.McpResultTooLarge]: "failed",
  // Nothing an app's call is answered with; failed if a gateway ever does.
  [ConversationErrorCode.AgentNotConfigured]: "failed",
  [ConversationErrorCode.AgentUnsupported]: "failed",
  [ConversationErrorCode.ModelUnavailable]: "failed",
  [ConversationErrorCode.ApprovalModeUnavailable]: "failed",
  [ConversationErrorCode.ApprovalModeNotApplied]: "failed",
  [ConversationErrorCode.ApprovalModeUncertain]: "failed",
  [ConversationErrorCode.ApprovalRequestConflict]: "failed",
  [ConversationErrorCode.TurnRunning]: "failed",
  [ConversationErrorCode.ConversationsNotConfigured]: "failed",
  [ConversationErrorCode.UnknownMethod]: "failed",
  [ConversationErrorCode.InvalidRequest]: "failed",
  [ConversationErrorCode.ConversationCapacity]: "failed",
  [ConversationErrorCode.ConversationConfigurationChanged]: "failed",
  [ConversationErrorCode.ConversationStateUnreadable]: "failed",
  [ConversationErrorCode.ConversationStorageUnavailable]: "failed",
  [ConversationErrorCode.AuditUnavailable]: "failed",
  [ConversationErrorCode.SubmissionConflict]: "failed",
  [ConversationErrorCode.SubmissionUnresolved]: "failed",
  [ConversationErrorCode.StalePermission]: "failed",
  [ConversationErrorCode.AgentStartupDeadline]: "failed",
  [ConversationErrorCode.AgentOperationFailed]: "failed",
  [ConversationErrorCode.ImageInputUnsupported]: "failed",
  [ConversationErrorCode.AttachmentNotFound]: "failed",
  [ConversationErrorCode.AttachmentUnavailable]: "failed",
  [ConversationErrorCode.AttachmentCapacity]: "failed",
  [ConversationErrorCode.AttachmentStorageUnavailable]: "failed",
  [ConversationErrorCode.AttachmentCleanupUnavailable]: "failed",
  [ConversationErrorCode.ConversationErasureIncomplete]: "failed",
}

/** What an app is told for each refusal, by what it asked for. */
const refusalWords: Record<Refusal, { tool: string; resource: string }> = {
  declined: {
    tool: "The person declined this action",
    resource: "The person declined this action",
  },
  expired: { tool: "No one answered in time", resource: "No one answered in time" },
  withdrawn: {
    tool: "The request was withdrawn",
    resource: "The request was withdrawn",
  },
  "not-for-app": {
    tool: "This app may not use that tool",
    resource: "This app may not read that resource",
  },
  "too-large": {
    tool: `The arguments are larger than ${MAX_MCP_ARGUMENTS_BYTES / 1024} KiB`,
    resource: "The request is too large",
  },
}

const failed: ServerAnswer = { kind: "failed" }
const utf8 = new TextEncoder()

/** The port's answer for what a call threw: a refusal, a server gone, or a failure. */
function answerFor(error: unknown, asked: "tool" | "resource"): ServerAnswer {
  // The client narrows the gateway's code to the ones this build knows, or
  // none (`conversationErrorCode`): a code that is not one is no key here.
  if (!(error instanceof NessaMcpAppError) || error.code === undefined) {
    // No answer, one the client did not believe, or a fault around the call.
    // Logged as the error alone: the arguments and any ticket are not in it.
    console.error("An MCP App call failed", error)
    return failed
  }
  const outcome = outcomes[error.code]
  if (outcome === "busy") return { kind: "busy" }
  if (outcome === "server-gone") return { kind: "server-gone" }
  if (outcome === "failed")
    return error.remoteError
      ? {
          kind: "failed",
          error: { code: error.remoteError.code, message: error.remoteError.message },
        }
      : failed
  return { kind: "refused", reason: refusalWords[outcome.refused][asked] }
}

/** The bytes as text, or `undefined` when they are not UTF-8. */
function decoded(bytes: Uint8Array): string | undefined {
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes)
  } catch {
    return undefined
  }
}

/** The `ReadResourceResult` the bridge reads an app from (`model/resource.ts`): no ticket in it. */
function readResult(described: McpReadResourceResult, text: string): JsonObject {
  const { csp, permissions, domain, prefersBorder } = described
  return {
    contents: [
      {
        uri: described.uri,
        mimeType: described.mimeType,
        text,
        _meta: {
          ui: {
            csp: {
              connectDomains: [...csp.connectDomains],
              resourceDomains: [...csp.resourceDomains],
              frameDomains: [...csp.frameDomains],
              baseUriDomains: [...csp.baseUriDomains],
            },
            permissions: { ...permissions },
            ...(domain === undefined ? {} : { domain }),
            ...(prefersBorder === undefined ? {} : { prefersBorder }),
          },
        },
      },
    ],
  }
}

/** The app's own server, through the gateway's `client.mcpApps`. */
export function gatewayAppServer(mcpApps: McpAppsApi): McpAppServer {
  return {
    // The longest the client waits for `mcp.callTool`: a review, then the call.
    callWithin: MCP_APP_CALL_DEADLINE_MS,

    async callTool(address: AppAddress, tool: string, args: JsonObject) {
      const argumentsJson = JSON.stringify(args)
      // The client's published bound, as the client counts it, so the app is
      // told before anything is sent (A2, gate 13: consumed, not retyped).
      if (utf8.encode(argumentsJson).byteLength > MAX_MCP_ARGUMENTS_BYTES)
        return { kind: "refused", reason: refusalWords["too-large"].tool }
      try {
        const { resultJson } = await mcpApps.callTool(
          address.sessionId,
          address.app,
          address.server,
          tool,
          argumentsJson,
        )
        // The client believes only one JSON object here (`mcpCallToolResult`).
        return { kind: "ok", result: JSON.parse(resultJson) as JsonObject }
      } catch (error) {
        return answerFor(error, "tool")
      }
    },

    async readResource(address: AppAddress, uri: string, signal: AbortSignal) {
      let described: McpReadResourceResult
      try {
        described = await mcpApps.readResource(
          address.sessionId,
          address.app,
          address.server,
          uri,
        )
      } catch (error) {
        return answerFor(error, "resource")
      }
      // The mount was released while the read was out: nothing to fetch for (R6).
      if (signal.aborted) return failed
      let bytes: Uint8Array
      try {
        // Redeemed at once, and only here: the ticket is spent whatever happens.
        bytes = await mcpApps.fetchResource(
          described.ticket,
          { size: described.size, sha256: described.sha256 },
          { signal },
        )
      } catch (error) {
        // Used, expired, released, not the bytes described, or unreachable:
        // the app is not loaded (R4). The error never holds the ticket; an
        // abort is the mount's own end, not a fault.
        if (!(error instanceof NessaMcpResourceError && error.code === "aborted"))
          console.error("An MCP App's resource was not fetched", error)
        return failed
      }
      const text = decoded(bytes)
      return text === undefined
        ? failed
        : { kind: "ok", result: readResult(described, text) }
    },

    async release(address: AppAddress) {
      await mcpApps.releaseApp(address.sessionId, address.app)
    },
  }
}
