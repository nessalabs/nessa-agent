import { NessaRpcError } from "./rpc-error.js"
import {
  PairingErrorCode,
  type PairingErrorCode as PairingError,
} from "../generated/product.js"

/**
 * A code the owner pairing methods answer with, including the session codes
 * the route uses beside {@link PairingErrorCode}. Membership is the only way
 * a raw code becomes one of these.
 */
export const PairingRefusalCode = {
  ...PairingErrorCode,
  Forbidden: "forbidden",
  Unauthorized: "unauthorized",
  InvalidRequest: "invalid_request",
  TemporarilyUnavailable: "temporarily_unavailable",
} as const

export type PairingRefusalCode =
  (typeof PairingRefusalCode)[keyof typeof PairingRefusalCode]

const pairingCodes = new Set<string>(Object.values(PairingRefusalCode))

export function pairingRefusalCode(code: unknown): PairingRefusalCode | undefined {
  return typeof code === "string" && pairingCodes.has(code)
    ? (code as PairingRefusalCode)
    : undefined
}

/** The method a {@link NessaPairingError} answers. */
export type PairingMethod =
  | "pairing.create"
  | "pairing.pending"
  | "pairing.status"
  | "pairing.approve"
  | "pairing.deny"
  | "pairing.cancel"

/**
 * A pairing method that did not answer with a result.
 *
 * `refusal` is a code this build knows. It is `undefined` when there was no
 * answer, the answer was not one the schema describes, or the code is not
 * one of {@link PairingRefusalCode}. Nothing is retried for you: read again
 * to see where the enrollment stands.
 */
export class NessaPairingError extends Error {
  readonly refusal: PairingRefusalCode | undefined

  constructor(
    readonly method: PairingMethod,
    cause: unknown,
  ) {
    const refusal =
      cause instanceof NessaRpcError ? pairingRefusalCode(cause.code) : undefined
    super(refusal ? `${method} was refused (${refusal})` : `${method} failed`, { cause })
    this.refusal = refusal
    this.name = "NessaPairingError"
  }
}

export type { PairingError }
