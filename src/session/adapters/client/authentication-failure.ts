import {
  NessaConnectionClosedError,
  NessaCredentialUnavailableError,
  NessaRpcError,
} from "@nessa/client"
import { SessionHealthError } from "./dev-session"

export function isAuthenticationFailure(error: unknown): boolean {
  if (error instanceof NessaRpcError) return error.code === "unauthorized"
  return (
    error instanceof NessaConnectionClosedError &&
    [
      "authentication_failed",
      "credential_revoked",
      "credential_expired",
      "authorization_lost",
    ].includes(error.closeReason)
  )
}

/**
 * Whether a session could not be made because it is signed out: there was
 * no credential to present, or the gateway refused the one presented — at
 * the handshake or at the health probe that follows it (`SessionHealthError`
 * carries the probe's own failure). Anything else, such as no answer, is not
 * a sign-in problem.
 */
export function isSignedOut(error: unknown): boolean {
  const cause = error instanceof SessionHealthError ? error.cause : error
  return (
    cause instanceof NessaCredentialUnavailableError || isAuthenticationFailure(cause)
  )
}
