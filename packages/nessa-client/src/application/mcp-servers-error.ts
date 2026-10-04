import { NessaRpcError } from "./rpc-error.js"
import {
  McpServersErrorCode,
  type McpRemoteErrorDetails,
  type McpServersAuditUnavailableDetails,
  type McpServersInvalidDetails,
  type McpServersRevisionConflictDetails,
} from "../generated/product.js"
import { mcpRemoteErrorDetails } from "../protocol/mcp-app-validate.js"
import {
  mcpServersAuditUnavailableDetails,
  mcpServersErrorCode,
  mcpServersInvalidDetails,
  mcpServersRevisionConflictDetails,
} from "../protocol/mcp-servers-validate.js"

/**
 * What the gateway refused an mcpServers method with: its code, and for the
 * codes that carry them, their details, typed for that code. Details that are
 * not their code's shape are `undefined` — the refusal without its
 * explanation, not a different refusal.
 */
export type McpServersRefusal =
  | {
      code: typeof McpServersErrorCode.McpServersInvalid
      details: McpServersInvalidDetails | undefined
    }
  | {
      code: typeof McpServersErrorCode.McpServersRevisionConflict
      details: McpServersRevisionConflictDetails | undefined
    }
  | {
      code: typeof McpServersErrorCode.AuditUnavailable
      details: McpServersAuditUnavailableDetails | undefined
    }
  | {
      code: typeof McpServersErrorCode.McpServerRemoteError
      details: McpRemoteErrorDetails | undefined
    }
  | {
      code: Exclude<
        McpServersErrorCode,
        | typeof McpServersErrorCode.McpServersInvalid
        | typeof McpServersErrorCode.McpServersRevisionConflict
        | typeof McpServersErrorCode.AuditUnavailable
        | typeof McpServersErrorCode.McpServerRemoteError
      >
      details?: undefined
    }

/** The method a {@link NessaMcpServersError} answers. */
export type McpServersMethod =
  "mcpServers.list" | "mcpServers.save" | "mcpServers.remove" | "mcpServers.inspect"

/** The session's refusal of a caller its policy does not allow, before the method is dispatched. */
const forbiddenCode = "forbidden"

function refusalOf(cause: NessaRpcError): McpServersRefusal | undefined {
  const code = mcpServersErrorCode(cause.code)
  switch (code) {
    case undefined:
      return undefined
    case McpServersErrorCode.McpServersInvalid:
      return { code, details: mcpServersInvalidDetails(cause.details) }
    case McpServersErrorCode.McpServersRevisionConflict:
      return { code, details: mcpServersRevisionConflictDetails(cause.details) }
    case McpServersErrorCode.AuditUnavailable:
      return { code, details: mcpServersAuditUnavailableDetails(cause.details) }
    case McpServersErrorCode.McpServerRemoteError:
      return { code, details: mcpRemoteErrorDetails(cause.details) }
    default:
      return { code }
  }
}

/**
 * An mcpServers method that did not answer with a result.
 *
 * `refusal` is the gateway's typed refusal, its code narrowed by membership of
 * {@link McpServersErrorCode} and its details typed for that code; it is
 * `undefined` for anything else — no answer at all, an answer this client
 * does not believe, or a code this build does not know — and `cause` holds
 * what happened. `forbidden` says the session refused the caller before the
 * method was dispatched: this credential may not manage MCP servers.
 *
 * A save or remove without a refusal may or may not have been applied:
 * `mcpServers.list` shows where things stand. Nothing is retried for you.
 */
export class NessaMcpServersError extends Error {
  readonly refusal: McpServersRefusal | undefined
  readonly forbidden: boolean

  constructor(
    /** The method that was refused or failed. */
    readonly method: McpServersMethod,
    cause: unknown,
  ) {
    const refusal = cause instanceof NessaRpcError ? refusalOf(cause) : undefined
    const forbidden = cause instanceof NessaRpcError && cause.code === forbiddenCode
    super(
      refusal
        ? `${method} was refused (${refusal.code})`
        : forbidden
          ? `${method} is not allowed for this credential`
          : `${method} failed`,
      { cause },
    )
    this.refusal = refusal
    this.forbidden = forbidden
    this.name = "NessaMcpServersError"
  }

  /** The refusal's code, or `undefined` when there is no typed refusal. */
  get code(): McpServersErrorCode | undefined {
    return this.refusal?.code
  }
}
