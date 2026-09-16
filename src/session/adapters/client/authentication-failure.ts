import { NessaConnectionClosedError, NessaRpcError } from "@nessa/client"

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
