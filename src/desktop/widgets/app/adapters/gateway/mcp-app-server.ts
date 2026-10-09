/**
 * `McpAppServer` over the gateway (`client.mcpApps`, #348(b)): an app's
 * `tools/call` and `resources/read` go to its own server as the gateway's
 * `mcp.callTool` and `mcp.readResource`, and its mount is let go with
 * `mcp.releaseApp`. Who may call what — a tool hidden from apps, another
 * server's, a destructive tool the person must review — is the gateway's to
 * decide; this only says what it answered, in the port's typed outcomes (the
 * state table on #384, rows A1–A14, R1–R8).
 *
 * A resource's bytes are fetched with the single-use ticket `readResource`
 * answers, at once and once. The client's `fetchResource` holds them to the
 * described size and SHA-256 — the one place that is checked — and the ticket
 * goes no further than that call: nothing here keeps, logs or returns it
 * (`mcp-app-server.test.ts`, "the ticket").
 *
 * What the app sends — a tool's name, a resource's URI, the arguments — is
 * held to the gateway's bounds by asking the client, which owns them
 * (`mcpAppRequestProblem`), before anything is sent: past them, the app's
 * request is refused in the client's words, and nothing is logged (A2). So a
 * `TypeError` the client throws after that is from what the host built — its
 * address — and is a fault, logged. What the gateway judges invalid
 * (`invalid_request`, A13), and a request too large for it to take
 * (`NessaRequestTooLargeError`, A14), are the app's request refused. A read
 * whose mount was released fetches nothing more (R6).
 *
 * `callTool` and `readResource` settle with an outcome whatever the client
 * throws, so the bridge never mistakes one for a fault of this adapter
 * (`mcp-app-server.test.ts`, "A11, R4: callTool and readResource never
 * reject, whatever is thrown"). Only `release` rejects, when
 * the gateway did not take it.
 */
import {
  ConversationErrorCode,
  mcpAppDeadlines,
  mcpAppRequestProblem,
  NessaConversationControlError,
  NessaMcpAppError,
  NessaMcpResourceError,
  NessaRequestTooLargeError,
  type McpAppsApi,
  type McpReadResourceResult,
} from "@nessa/client"
import type { AppAddress, McpAppServer, ServerAnswer } from "../../application/ports"
import type { JsonObject } from "../../model/json-rpc"

/** What the gateway refused, in words the app is shown. */
type Refusal =
  | "declined"
  | "expired"
  | "withdrawn"
  | "not-for-app"
  | "too-large"
  | "invalid"
  | "turn-running"

/** What one of the gateway's codes comes to (gate 11: every code is placed). */
type Outcome = { readonly refused: Refusal } | "busy" | "server-gone" | "failed"

/**
 * Each of the gateway's codes, as an app is answered for it: the one table,
 * for its calls to its server here and, through `answerFor`, for its
 * conversation's messages and context (`app-messages.ts`, #390).
 */
const outcomes: Readonly<Record<ConversationErrorCode, Outcome>> = {
  [ConversationErrorCode.McpApprovalDenied]: { refused: "declined" },
  [ConversationErrorCode.McpApprovalExpired]: { refused: "expired" },
  [ConversationErrorCode.McpCancelled]: { refused: "withdrawn" },
  [ConversationErrorCode.McpToolNotForApp]: { refused: "not-for-app" },
  [ConversationErrorCode.McpServerMismatch]: { refused: "not-for-app" },
  [ConversationErrorCode.McpAppUnknown]: { refused: "not-for-app" },
  [ConversationErrorCode.McpRequestTooLarge]: { refused: "too-large" },
  // No room on the app lane; nothing reached the server (L1b, A8).
  [ConversationErrorCode.TemporarilyUnavailable]: "busy",
  // The gateway judged the request itself — arguments that are no JSON
  // object, a URI that is no app's — before dispatch: the app's to fix.
  [ConversationErrorCode.InvalidRequest]: { refused: "invalid" },
  // An app's message while the agent is at work, or input waits: nothing
  // was sent, and the app may send it again once the turn is done (M11).
  [ConversationErrorCode.TurnRunning]: { refused: "turn-running" },
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
  // The session was not reported as ended. These are a refusal, a miss, or a
  // scope the token does not carry — never "the server stopped".
  [ConversationErrorCode.McpUnauthorized]: "failed",
  [ConversationErrorCode.McpUnreachable]: "failed",
  [ConversationErrorCode.McpInsufficientScope]: "failed",
  // Nothing an app's call is answered with; failed if a gateway ever does.
  [ConversationErrorCode.AgentNotConfigured]: "failed",
  [ConversationErrorCode.AgentUnsupported]: "failed",
  [ConversationErrorCode.ModelUnavailable]: "failed",
  [ConversationErrorCode.ApprovalModeUnavailable]: "failed",
  [ConversationErrorCode.ApprovalModeNotApplied]: "failed",
  [ConversationErrorCode.ApprovalModeUncertain]: "failed",
  [ConversationErrorCode.ApprovalRequestConflict]: "failed",
  [ConversationErrorCode.ConversationsNotConfigured]: "failed",
  [ConversationErrorCode.UnknownMethod]: "failed",
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

/** What an app asked for: its server's tool or resource, or its conversation. */
export type Asked = "tool" | "resource" | "conversation"

/** The same words, whatever the app asked for. */
const said = (words: string): Record<Asked, string> => ({
  tool: words,
  resource: words,
  conversation: words,
})

/** What an app is told for each refusal, by what it asked for. */
const refusalWords: Record<Refusal, Record<Asked, string>> = {
  declined: said("The person declined this action"),
  expired: said("No one answered in time"),
  withdrawn: said("The request was withdrawn"),
  "not-for-app": {
    tool: "This app may not use that tool",
    resource: "This app may not read that resource",
    conversation: "This app may not speak in this conversation",
  },
  "too-large": said("The request is larger than the gateway accepts"),
  invalid: said("The gateway refused the request as invalid"),
  "turn-running": said("The conversation is busy"),
}

const failed: ServerAnswer = { kind: "failed" }

/** The conversation is gone: a release finds nothing left to let go (M5). */
const conversationGone: ReadonlySet<ConversationErrorCode | undefined> = new Set([
  ConversationErrorCode.ConversationNotFound,
  ConversationErrorCode.ConversationDeleted,
])

/**
 * The port's answer for what a call threw: a refusal, a server gone, or a
 * failure, which is logged. Shared with the conversation's port
 * (`app-messages.ts`), so a code means one thing to an app whatever it asked.
 */
export function answerFor(error: unknown, asked: Asked): ServerAnswer {
  // Too large to send at all: the client refused it, and nothing was sent.
  if (
    error instanceof NessaMcpAppError &&
    error.cause instanceof NessaRequestTooLargeError
  )
    return { kind: "refused", reason: refusalWords["too-large"][asked] }
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

/**
 * Whether trying a release again may change the outcome. Not a release the
 * client refused to send (`TypeError`: an address it cannot carry), and not
 * one the gateway refused before applying it (`uncertain` false). A lost
 * answer, a closed socket, a timeout or a busy gateway (`temporarily_unavailable`,
 * which the client counts as uncertain) may all have left it untaken.
 */
function releaseMayLand(error: unknown): boolean {
  if (error instanceof TypeError) return false
  return !(error instanceof NessaConversationControlError) || error.uncertain
}

/**
 * How long to wait before each try of a release after the first: 10.5 s of
 * waiting in all, plus each try's own request timeout. A release is
 * idempotent, so a lost or busy answer is asked again. A review the person
 * has not answered expires on its own deadline whatever happens here, and the
 * gateway withdraws a mount's reviews when its socket closes.
 */
export const releaseRetryMs: readonly number[] = Object.freeze([500, 2_000, 8_000])

/**
 * The app's own server, through the gateway's `client.mcpApps`. `after` is
 * the clock a release that did not land waits on before it is tried again,
 * and `newId` mints the one request id every try of a release carries.
 */
export function gatewayAppServer(
  mcpApps: McpAppsApi,
  after: (ms: number, run: () => void) => () => void,
  newId: () => string,
): McpAppServer {
  return {
    // The longest the client waits for `mcp.callTool`: a review, then the call.
    callWithin: mcpAppDeadlines.callToolMs,

    async callTool(address: AppAddress, tool: string, args: JsonObject) {
      const argumentsJson = JSON.stringify(args)
      // Asked of the client, the bounds' owner, before anything is sent (A2).
      const problem =
        mcpAppRequestProblem.tool(tool) ??
        mcpAppRequestProblem.argumentsJson(argumentsJson)
      if (problem) return { kind: "refused", reason: problem }
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
      const problem = mcpAppRequestProblem.uri(uri)
      if (problem) return { kind: "refused", reason: problem }
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
        // the app is not loaded (R4). The error never holds the ticket.
        // `aborted` is not logged: the client answers it only for the signal
        // it was given, the mount's, so it is the mount's own end. Its own
        // deadline is `timeout`, and a transport's own AbortError is
        // `unreachable` (`mcp-apps-api.test.ts`: "reports the caller's own
        // abort as aborted…", "gives up on a fetch that never answers…" and
        // "reports a transport's own AbortError, the caller's signal still
        // live, as unreachable…"). The code is taken as the client gives it,
        // not checked against the signal: an `aborted` while the mount is
        // live — which that client cannot answer — is a failure, unlogged (R8).
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
      // One teardown, one action: every try carries the same request id.
      const requestId = newId()
      for (let tried = 0; ; tried++) {
        try {
          await mcpApps.releaseApp(address.sessionId, address.app, { requestId })
          return
        } catch (error) {
          // A conversation that is gone has no mount left to release: the
          // expected end of one, not a fault.
          if (
            error instanceof NessaConversationControlError &&
            conversationGone.has(error.code)
          )
            return
          // Until the gateway has taken it, the mount's reviews stay open,
          // so a release that may not have landed is asked again (#384's
          // rows M5 to M5e).
          const wait = releaseRetryMs[tried]
          if (!releaseMayLand(error) || wait === undefined) throw error
          await new Promise<void>((resolve) => after(wait, resolve))
        }
      }
    },
  }
}
