/** Failed mutation attempt. Retain requestId for recovery after an uncertain transport failure. Inspect code/cause first: credential_conflict, credential_capacity, and credential_not_found are command rejections, not transient failures. */
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
