import { NessaRpcError } from "./rpc-error.js"
import { LoginsErrorCode } from "../generated/product.js"

/**
 * A code the `logins.*` methods answer with, including the session codes the
 * route uses beside {@link LoginsErrorCode}. Membership is the only way a
 * raw code becomes one of these.
 */
export const LoginsRefusalCode = {
  ...LoginsErrorCode,
  Forbidden: "forbidden",
  Unauthorized: "unauthorized",
  InvalidRequest: "invalid_request",
  TemporarilyUnavailable: "temporarily_unavailable",
} as const

export type LoginsRefusalCode = (typeof LoginsRefusalCode)[keyof typeof LoginsRefusalCode]

const loginsCodes = new Set<string>(Object.values(LoginsRefusalCode))

export function loginsRefusalCode(code: unknown): LoginsRefusalCode | undefined {
  return typeof code === "string" && loginsCodes.has(code)
    ? (code as LoginsRefusalCode)
    : undefined
}

/** The method a {@link NessaLoginsError} answers. */
export type LoginsMethod =
  "logins.status" | "logins.save" | "logins.remove" | "logins.setDevice"

/**
 * A `logins.*` method that did not answer with a result.
 *
 * `refusal` is a code this build knows, or `undefined` when there was no
 * answer, the answer was not one the schema describes, or the code is not
 * one of {@link LoginsRefusalCode}. Nothing is retried for you.
 */
export class NessaLoginsError extends Error {
  readonly refusal: LoginsRefusalCode | undefined

  constructor(
    readonly method: LoginsMethod,
    cause: unknown,
  ) {
    const refusal =
      cause instanceof NessaRpcError ? loginsRefusalCode(cause.code) : undefined
    super(refusal ? `${method} was refused (${refusal})` : `${method} failed`, { cause })
    this.refusal = refusal
    this.name = "NessaLoginsError"
  }
}
