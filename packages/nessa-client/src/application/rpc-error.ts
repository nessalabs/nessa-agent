/** Structured RPC failure from a `type: "res"` error frame. */
export class NessaRpcError extends Error {
  readonly code: string
  readonly details: unknown | undefined

  constructor(code: string, message: string, details?: unknown) {
    super(message)
    this.name = "NessaRpcError"
    this.code = code
    this.details = details
  }
}
