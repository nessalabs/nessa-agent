import { conversationErrorCode } from "./conversation-error-code.js"
import { rejectedBeforeDispatch } from "./conversation-mutation-error.js"
import { NessaRpcError } from "./rpc-error.js"
import {
  ConversationErrorCode,
  mcpAppTiming,
  type McpAppReference,
  type McpRemoteErrorDetails,
} from "../generated/product.js"
import { mcpRemoteErrorDetails } from "../protocol/mcp-app-validate.js"

/**
 * The longest `mcp.callTool` can take the gateway: a destructive tool's review
 * waits for the person, then the call itself has its budget, and the client's
 * allowance covers audit writes, the response, and scheduling. A client that
 * gave up sooner would drop an answer the gateway still sends — and giving up
 * does not withdraw the review, which only the socket closing does. The
 * protocol publishes each part and their sum (`x-mcpAppTiming`).
 */
export const MCP_APP_CALL_DEADLINE_MS = mcpAppTiming.callDeadlineMs

/**
 * The longest `mcp.readResource` can take the gateway: the server's budget for
 * the read, and the client's allowance (`x-mcpAppTiming`). Nothing waits on the
 * person, so a client configured for less would give up on a read the gateway
 * is still bound to answer.
 */
export const MCP_APP_READ_DEADLINE_MS = mcpAppTiming.readDeadlineMs

/**
 * An MCP App's call (`mcp.callTool`, `mcp.readResource`) that did not answer.
 *
 * `uncertain` is false only when the gateway refused the call before anything
 * reached the app's server, so the tool certainly had no effect:
 * `mcp_app_unknown`, `mcp_server_mismatch`, `mcp_tool_not_for_app`,
 * `mcp_request_too_large`, `mcp_approval_denied`, `mcp_approval_expired`,
 * `mcp_cancelled`, and the conversation refusals that come before any command
 * (`invalid_request`, `conversation_not_found`, …). The server may have been
 * asked for `mcp_session_unavailable`, `mcp_timed_out`, `mcp_remote_error` and
 * `mcp_result_too_large`, and for anything without a code — no answer at all,
 * or one this client does not believe. Nothing is retried for you.
 */
export class NessaMcpAppError extends Error {
  /** Typed conversation rejection code, or undefined for a transport failure, a malformed answer, or a code this build does not know. Inspect `cause` for those. */
  readonly code: ConversationErrorCode | undefined
  /** False only when the gateway refused before anything reached the app's server. */
  readonly uncertain: boolean
  /** The server's own JSON-RPC error, when `code` is `mcp_remote_error` and it sent one. Absent for an answer that was no MCP answer at all. */
  readonly remoteError: McpRemoteErrorDetails | undefined

  constructor(
    /** Conversation the app is in. */
    readonly conversationId: string,
    /** Action identity the call was sent with. */
    readonly requestId: string,
    /** The app that asked. */
    readonly app: McpAppReference,
    cause: unknown,
  ) {
    const code =
      cause instanceof NessaRpcError ? conversationErrorCode(cause.code) : undefined
    super(code ? `MCP App call was refused (${code})` : "MCP App call failed", { cause })
    this.code = code
    this.uncertain = !(code !== undefined && rejectedBeforeDispatch(code))
    this.remoteError =
      code === ConversationErrorCode.McpRemoteError
        ? mcpRemoteErrorDetails((cause as NessaRpcError).details)
        : undefined
    this.name = "NessaMcpAppError"
  }
}
