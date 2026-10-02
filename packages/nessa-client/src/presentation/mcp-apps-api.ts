import { NessaConversationControlError } from "../application/conversation-mutation-error.js"
import { requestWithin, type RequestTimer } from "../application/gateway-http.js"
import {
  MCP_APP_CALL_DEADLINE_MS,
  NessaMcpAppError,
} from "../application/mcp-app-call.js"
import {
  NessaMcpResourceError,
  RESOURCE_DEADLINE_MS,
  type McpResourceTransport,
} from "../application/mcp-resource-fetch.js"
import type { RequestDeadline, RpcRequester } from "../application/session-port.js"
import {
  bounds,
  ProductMethod,
  type ConversationMutationResult,
  type McpAppReference,
  type McpCallToolResult,
  type McpReadResourceResult,
} from "../generated/product.js"
import {
  conversationIdPattern,
  conversationMutation,
} from "../protocol/conversation-validate.js"
import {
  boundedName,
  mcpAppReferenceProblem,
  mcpAppRequestProblem,
  mcpCallToolResult,
  mcpReadResourceResult,
  validResourceDigest,
  validResourceSize,
  validResourceTicket,
} from "../protocol/mcp-app-validate.js"
import type { ConversationActionOptions } from "./conversation-api.js"

/** What `mcp.readResource` said the bytes are: the size and SHA-256 `fetchResource` holds them to. */
export type McpResourceDescription = Pick<McpReadResourceResult, "size" | "sha256">

/**
 * An MCP App's calls, made by the host that shows it.
 *
 * An app is the UI of one MCP tool call in a conversation, named by an
 * {@link McpAppReference}: the call's `executionId` and `toolId`, and the host's
 * own `instanceId` for this mount of it. It reaches its own server — and no
 * other — through `callTool` and `readResource`; the host lets it go with
 * `releaseApp` when the mount is torn down.
 *
 * A resource's bytes never ride the socket. `readResource` answers what they
 * are and a single-use ticket; `fetchResource` redeems it over HTTP and hands
 * back the bytes only once their size and SHA-256 are the ones described.
 *
 * Every argument is checked against the schema's bounds before anything is
 * sent, and every answer against the schema before it is believed. Nothing is
 * retried for you.
 */
export type McpAppsApi = {
  /**
   * Call a tool on the app's own server, as the conversation's own session.
   *
   * A destructive tool — one whose `readOnlyHint` is not true and whose
   * `destructiveHint` is not false, so a tool with no annotations is — waits
   * first for the person's approval, as a review in the conversation's
   * `permissions` with `origin: {kind: "app", server, tool}`. The call is
   * answered when they answer, when the review expires after 5 minutes, or
   * when it is withdrawn; this client waits that long for it. At most 4 app
   * calls run at once per socket; past that they are refused
   * `temporarily_unavailable`.
   * @param conversationId - Canonical lowercase UUID of the app's conversation.
   * @param app - The app asking: its tool call and this mount of it.
   * @param server - The app's own server, by its configured name: 1-128 UTF-8 bytes.
   * @param tool - The tool on that server: 1-128 UTF-8 bytes.
   * @param argumentsJson - The tool's arguments, one JSON object encoded, at
   * most 32 KiB of UTF-8 — the most a review shows. Omitted is none. Whether it
   * is an object is the gateway's to decide (`invalid_request`).
   * @param options - Optional caller-managed action identity.
   * @returns The server's `CallToolResult`, encoded, exactly as it answered.
   * `isError: true` inside it is a result for the app, not a refusal.
   * @throws TypeError for arguments outside the schema's bounds, before
   * anything is sent; otherwise {@link NessaMcpAppError}. Its `uncertain` is
   * false — nothing reached the server — for `mcp_app_unknown`,
   * `mcp_server_mismatch`, `mcp_tool_not_for_app`, `mcp_request_too_large`,
   * `mcp_approval_denied`, `mcp_approval_expired` and `mcp_cancelled`. The
   * server may have been asked for `mcp_session_unavailable`,
   * `mcp_timed_out`, `mcp_remote_error` (with `remoteError` when the server
   * sent a JSON-RPC error) and `mcp_result_too_large`.
   */
  callTool: (
    conversationId: string,
    app: McpAppReference,
    server: string,
    tool: string,
    argumentsJson?: string,
    options?: ConversationActionOptions,
  ) => Promise<McpCallToolResult>
  /**
   * Read a resource of the app's own server: once, held by the gateway as
   * exactly those bytes, and described with a ticket to fetch them.
   * @param conversationId - Canonical lowercase UUID of the app's conversation.
   * @param app - The app asking: its tool call and this mount of it.
   * @param server - The app's own server, by its configured name: 1-128 UTF-8 bytes.
   * @param uri - The resource's URI on that server: 1-2048 UTF-8 bytes.
   * @param options - Optional caller-managed action identity.
   * @returns What the bytes are — always an MCP App's HTML, at most 4 MiB —
   * with the CSP and permissions the app asked for, and a `ticket` for
   * `fetchResource`: secret, single use, and redeemable for `expiresInMs`
   * (60 s). Never log it or put it in a URL.
   * @throws TypeError for arguments outside the schema's bounds, before
   * anything is sent; otherwise {@link NessaMcpAppError}, with `uncertain`
   * false for a refusal made before anything reached the server, such as
   * `mcp_app_unknown` and `mcp_server_mismatch`. An `mcp_app_unknown` for a
   * resource that is not an app's HTML was read, which changes nothing. The
   * server was asked for `mcp_session_unavailable`, `mcp_timed_out` and
   * `mcp_remote_error`.
   */
  readResource: (
    conversationId: string,
    app: McpAppReference,
    server: string,
    uri: string,
    options?: ConversationActionOptions,
  ) => Promise<McpReadResourceResult>
  /**
   * Redeem a resource ticket: `GET /mcp-resources` on the gateway's own HTTP
   * origin, the ticket in its header and never in the URL.
   * @param ticket - From `readResource`. Spent by the request, whatever its outcome.
   * @param expected - The `size` and `sha256` `readResource` answered with it.
   * @param options - `signal` abandons the request, which then fails as `aborted`.
   * @returns Exactly the bytes described, once their length and SHA-256 match.
   * @throws TypeError for a malformed ticket or description, before any
   * request; otherwise {@link NessaMcpResourceError}: `not_found` (the route's
   * one refusal: unknown, used, expired or released), `unavailable` (the
   * redemption could not be audited), `integrity` (bytes that are not the
   * ones described), `aborted`, `timeout` (no answer within 60 s),
   * `unreachable`, or `unexpected_response`.
   */
  fetchResource: (
    ticket: string,
    expected: McpResourceDescription,
    options?: { signal?: AbortSignal },
  ) => Promise<Uint8Array<ArrayBuffer>>
  /**
   * Say the host tore this mount of an app down. Every review it has open is
   * withdrawn — its waiting call answers `mcp_cancelled` — and every resource
   * ticket issued to it is released. Releasing a mount with nothing open
   * succeeds too, and repeating it is harmless. It never waits behind the
   * app's own calls.
   * @param conversationId - Canonical lowercase UUID of the app's conversation.
   * @param app - The mount torn down.
   * @param options - Optional caller-managed action identity.
   * @returns The gateway's acknowledgement for this action.
   * @throws TypeError for arguments outside the schema's bounds, before
   * anything is sent; otherwise NessaConversationControlError, whose
   * `uncertain` is false only for a refusal made before anything was released.
   */
  releaseApp: (
    conversationId: string,
    app: McpAppReference,
    options?: ConversationActionOptions,
  ) => Promise<ConversationMutationResult>
}

const utf8 = new TextEncoder()
const callDeadline: RequestDeadline = { atLeastMs: MCP_APP_CALL_DEADLINE_MS }

function hex(digest: ArrayBuffer): string {
  return Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("")
}

export function createMcpAppsApi(
  session: RpcRequester,
  transport: McpResourceTransport,
  newId: () => string,
  timer: RequestTimer,
): McpAppsApi {
  /** The checked identities every call carries, and a copy of the app the caller cannot change under it. */
  function addressed(
    conversationId: string,
    app: McpAppReference,
    options: ConversationActionOptions,
  ) {
    if (!conversationIdPattern.test(conversationId))
      throw new TypeError("Conversation ID must be a canonical lowercase UUID")
    const requestId = options.requestId ?? newId()
    if (!requestId.trim() || utf8.encode(requestId).byteLength > 256)
      throw new TypeError("Request ID must contain 1-256 UTF-8 bytes")
    const problem = mcpAppReferenceProblem(app)
    if (problem) throw new TypeError(`Invalid app: ${problem}`)
    const { executionId, toolId, instanceId } = app
    return {
      conversationId,
      requestId,
      app: Object.freeze({ executionId, toolId, instanceId }),
    }
  }
  function serverName(server: string) {
    if (!boundedName(server, bounds.maxMcpNameBytes))
      throw new TypeError(`Server must contain 1-${bounds.maxMcpNameBytes} UTF-8 bytes`)
  }
  async function call<T>(
    method: string,
    command: ReturnType<typeof addressed> & Record<string, unknown>,
    validate: (value: unknown) => T,
    deadline?: RequestDeadline,
  ): Promise<T> {
    try {
      return validate(
        deadline === undefined
          ? await session.request(method, command)
          : await session.request(method, command, deadline),
      )
    } catch (cause) {
      throw new NessaMcpAppError(
        command.conversationId,
        command.requestId,
        command.app,
        cause,
      )
    }
  }
  return {
    async callTool(conversationId, app, server, tool, argumentsJson, options = {}) {
      const command = addressed(conversationId, app, options)
      serverName(server)
      const toolProblem =
        typeof tool === "string"
          ? mcpAppRequestProblem.tool(tool)
          : "Tool must be a string"
      if (toolProblem) throw new TypeError(toolProblem)
      if (argumentsJson !== undefined) {
        if (typeof argumentsJson !== "string")
          throw new TypeError("Arguments must be one JSON object, encoded")
        const problem = mcpAppRequestProblem.argumentsJson(argumentsJson)
        if (problem) throw new TypeError(problem)
      }
      return call(
        ProductMethod.McpCallTool,
        {
          ...command,
          server,
          tool,
          ...(argumentsJson === undefined ? {} : { argumentsJson }),
        },
        mcpCallToolResult,
        callDeadline,
      )
    },
    async readResource(conversationId, app, server, uri, options = {}) {
      const command = addressed(conversationId, app, options)
      serverName(server)
      const uriProblem =
        typeof uri === "string"
          ? mcpAppRequestProblem.uri(uri)
          : "Resource URI must be a string"
      if (uriProblem) throw new TypeError(uriProblem)
      return call(ProductMethod.McpReadResource, { ...command, server, uri }, (value) =>
        mcpReadResourceResult(value, uri),
      )
    },
    async fetchResource(ticket, expected, options = {}) {
      if (!validResourceTicket(ticket))
        throw new TypeError("Resource ticket must be 43 base64url characters")
      if (!validResourceSize(expected.size))
        throw new TypeError(`Resource size must be 0-${bounds.maxMcpResourceBytes} bytes`)
      if (!validResourceDigest(expected.sha256))
        throw new TypeError("Resource sha256 must be 64 lowercase hexadecimal digits")
      const reply = await requestWithin(
        (signal) => transport.get({ ticket, maxBytes: expected.size, signal }),
        options.signal,
        timer,
        RESOURCE_DEADLINE_MS,
        {
          aborted: () => new NessaMcpResourceError("aborted"),
          timeout: () => new NessaMcpResourceError("timeout"),
          unreachable: (cause) =>
            new NessaMcpResourceError("unreachable", undefined, cause),
        },
      )
      if (reply.status === 404) throw new NessaMcpResourceError("not_found", 404)
      if (reply.status === 503) throw new NessaMcpResourceError("unavailable", 503)
      if (reply.status !== 200)
        throw new NessaMcpResourceError("unexpected_response", reply.status)
      // Checked before anyone renders them: bytes that are not the ones the
      // socket described are no resource at all, however they arrived.
      if (reply.bytes.byteLength !== expected.size)
        throw new NessaMcpResourceError("integrity", 200)
      const digest = hex(await crypto.subtle.digest("SHA-256", reply.bytes))
      if (digest !== expected.sha256) throw new NessaMcpResourceError("integrity", 200)
      return reply.bytes
    },
    async releaseApp(conversationId, app, options = {}) {
      const command = addressed(conversationId, app, options)
      try {
        return conversationMutation(
          await session.request(ProductMethod.McpReleaseApp, command),
          command.requestId,
        )
      } catch (cause) {
        throw new NessaConversationControlError(
          command.conversationId,
          command.requestId,
          command.app.executionId,
          cause,
        )
      }
    },
  }
}
