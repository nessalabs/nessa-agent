import type { NessaConnectionClosedError } from "./connection-closed-error.js"

/** Narrow request dependency used by typed RPC namespaces. */
export interface RpcRequester {
  /**
   * Send one RPC. `timeoutMs` overrides the connection's default deadline for
   * operations the gateway can legitimately spend longer on than a normal
   * request — opening an agent is the one that matters — so the client does
   * not abandon a command the gateway is still answering.
   */
  request(method: string, params: unknown, timeoutMs?: number): Promise<unknown>
}

/** Replaceable transport owned by a client connection. */
export interface SessionTransport extends RpcRequester {
  readonly termination: NessaConnectionClosedError | undefined
  onEvent(event: string, handler: (payload: unknown) => void): () => void
  onClose(handler: (error: NessaConnectionClosedError) => void): () => void
  close(): void
}
