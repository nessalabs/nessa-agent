import type { NessaConnectionClosedError } from "./connection-closed-error.js"

/**
 * How one request's deadline differs from the connection's configured default.
 *
 * Both directions are real, and neither may quietly replace the caller's own
 * setting: authentication must not outlive the challenge it answers, and a
 * conversation command must not be abandoned while the gateway is still
 * launching an agent. A bound is therefore applied to the configured value
 * rather than instead of it.
 */
export type RequestDeadline = {
  /** Never wait longer than this, however long the connection is configured for. */
  atMostMs?: number
  /** Wait at least this long, even if the connection is configured for less. */
  atLeastMs?: number
}

/** Narrow request dependency used by typed RPC namespaces. */
export interface RpcRequester {
  /** Send one RPC, optionally bounding this request's deadline. */
  request(method: string, params: unknown, deadline?: RequestDeadline): Promise<unknown>
}

/** Replaceable transport owned by a client connection. */
export interface SessionTransport extends RpcRequester {
  readonly termination: NessaConnectionClosedError | undefined
  onEvent(event: string, handler: (payload: unknown) => void): () => void
  onClose(handler: (error: NessaConnectionClosedError) => void): () => void
  close(): void
}
