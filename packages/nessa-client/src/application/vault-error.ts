import { NessaRpcError } from "./rpc-error.js"
import { VaultErrorCode } from "../generated/product.js"
/** Redacted refusal from a vault operation; secret parameters are never retained. */
export class NessaVaultError extends Error {
  readonly refusal: VaultErrorCode | undefined
  constructor(cause: unknown) {
    const code = cause instanceof NessaRpcError ? cause.code : undefined
    const refusal = Object.values(VaultErrorCode).find((known) => known === code)
    super(refusal ? `Vault operation refused (${refusal})` : "Vault unavailable")
    this.name = "NessaVaultError"
    this.refusal = refusal
  }
}
