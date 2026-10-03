import { NessaConnectionClosedError, NessaRpcError } from "@nessa/client"
import { attemptFailure } from "./dev-session"

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
 * Whether a session could not be made because it is signed out: the gateway
 * refused the credential presented, at the handshake or at the health probe
 * that follows it (`attemptFailure`). A browser surface with no session for
 * its origin is refused the same way: `connectBrowserSession` answers
 * `unauthorized` itself. Having no credential to present at all is not this:
 * in the desktop app that is a configuration fault — a gateway URL that is
 * not loopback — which signing in would not repair. Anything
 * else, such as no answer, is not a sign-in problem either.
 */
export function isSignedOut(error: unknown): boolean {
  return isAuthenticationFailure(attemptFailure(error))
}
