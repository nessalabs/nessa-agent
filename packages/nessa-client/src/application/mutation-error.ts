/** Failed mutation attempt. Persist requestId and reuse it for an explicit retry. */
export class NessaMutationError extends Error {
  constructor(
    readonly requestId: string,
    cause: unknown,
  ) {
    super(`Credential mutation failed (requestId: ${requestId})`, { cause })
    this.name = "NessaMutationError"
  }

  /** Original RPC or transport code, when one was available. */
  get code(): string | number | undefined {
    const cause = this.cause
    if (
      typeof cause === "object" &&
      cause !== null &&
      "code" in cause &&
      (typeof cause.code === "string" || typeof cause.code === "number")
    )
      return cause.code
    return undefined
  }
}
