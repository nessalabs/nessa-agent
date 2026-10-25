import { NessaRpcError } from "./rpc-error.js"
import { GithubErrorCode } from "../generated/product.js"

/**
 * A code the `github.*` methods answer with, including the session codes the
 * route uses beside {@link GithubErrorCode}. Membership is the only way a raw
 * code becomes one of these.
 */
export const GithubRefusalCode = {
  ...GithubErrorCode,
  Forbidden: "forbidden",
  Unauthorized: "unauthorized",
  InvalidRequest: "invalid_request",
  TemporarilyUnavailable: "temporarily_unavailable",
} as const

export type GithubRefusalCode = (typeof GithubRefusalCode)[keyof typeof GithubRefusalCode]

const githubCodes = new Set<string>(Object.values(GithubRefusalCode))

export function githubRefusalCode(code: unknown): GithubRefusalCode | undefined {
  return typeof code === "string" && githubCodes.has(code)
    ? (code as GithubRefusalCode)
    : undefined
}

/** The method a {@link NessaGithubError} answers. */
export type GithubMethod =
  "github.status" | "github.connect" | "github.cancel" | "github.disconnect"

/**
 * A `github.*` method that did not answer with a result.
 *
 * `refusal` is a code this build knows. It is `undefined` when there was no
 * answer, the answer was not one the schema describes, or the code is not
 * one of {@link GithubRefusalCode}. Nothing is retried for you: read
 * `github.status` to see where the sign-in stands.
 */
export class NessaGithubError extends Error {
  readonly refusal: GithubRefusalCode | undefined

  constructor(
    readonly method: GithubMethod,
    cause: unknown,
  ) {
    const refusal =
      cause instanceof NessaRpcError ? githubRefusalCode(cause.code) : undefined
    super(refusal ? `${method} was refused (${refusal})` : `${method} failed`, { cause })
    this.refusal = refusal
    this.name = "NessaGithubError"
  }
}
