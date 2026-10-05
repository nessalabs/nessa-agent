import { conversationErrorCode } from "./conversation-error-code.js"
import { rejectedBeforeDispatch } from "./conversation-mutation-error.js"
import { NessaRequestTooLargeError } from "./request-too-large-error.js"
import { NessaRpcError } from "./rpc-error.js"
import {
  ConversationErrorCode,
  type McpAppReference,
  type McpRemoteErrorDetails,
} from "../generated/product.js"
import { mcpRemoteErrorDetails } from "../protocol/mcp-app-validate.js"

/**
 * An MCP App's request (`mcp.callTool`, `mcp.readResource`, `mcp.sendMessage`,
 * `mcp.updateModelContext`) that did not answer.
 *
 * `uncertain` is false only when the gateway refused the request before it
 * could take effect — for a call, before anything reached the app's server;
 * for a message, before it became a turn; for a context, before it was held:
 * `mcp_app_unknown`, `mcp_server_mismatch`, `mcp_tool_not_for_app`,
 * `mcp_request_too_large`, `mcp_approval_denied`, `mcp_approval_expired`,
 * `mcp_cancelled`, `turn_running`, and the conversation refusals that come
 * before any command (`invalid_request`, `conversation_not_found`, …); and
 * for a request this client would not send, too large for the gateway to
 * take — its `cause` a {@link NessaRequestTooLargeError}. It may have taken
 * effect for `mcp_session_unavailable`, `mcp_timed_out`, `mcp_remote_error`
 * and `mcp_result_too_large`, for the codes no command can be sure of
 * (`temporarily_unavailable`, `audit_unavailable`, `submission_unresolved`,
 * …), and for anything without a code — no answer at all, or one this client
 * does not believe. Nothing is retried for you.
 */
export class NessaMcpAppError extends Error {
  /** Typed conversation rejection code, or undefined for a transport failure, a malformed answer, or a code this build does not know. Inspect `cause` for those. */
  readonly code: ConversationErrorCode | undefined
  /** False only when the gateway refused before the request could take effect. */
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
    // Nothing was sent for a request too large to send, nor for a refusal
    // made before dispatch.
    this.uncertain =
      !(cause instanceof NessaRequestTooLargeError) &&
      !(code !== undefined && rejectedBeforeDispatch(code))
    this.remoteError =
      code === ConversationErrorCode.McpRemoteError
        ? mcpRemoteErrorDetails((cause as NessaRpcError).details)
        : undefined
    this.name = "NessaMcpAppError"
  }
}
