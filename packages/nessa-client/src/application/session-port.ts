import type { NessaConnectionClosedError } from "./connection-closed-error.js"

/** Narrow request dependency used by typed RPC namespaces. */
export interface RpcRequester {
  request(method: string, params: unknown): Promise<unknown>
}

/** Replaceable transport owned by a client connection. */
export interface SessionTransport extends RpcRequester {
  readonly termination: NessaConnectionClosedError | undefined
  onEvent(event: string, handler: (payload: unknown) => void): () => void
  onClose(handler: (error: NessaConnectionClosedError) => void): () => void
  close(): void
}
