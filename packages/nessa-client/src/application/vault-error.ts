import { NessaRpcError } from "./rpc-error.js"
import { VaultErrorCode } from "../generated/product.js"
/** Redacted refusal from a vault operation; secret parameters are never retained. */
export class NessaVaultError extends Error {
  readonly refusal: VaultErrorCode | undefined
  constructor(cause: unknown, changeDispatched = false) {
    const code =
      cause instanceof NessaRpcError
        ? cause.code === "forbidden"
          ? VaultErrorCode.VaultForbidden
          : cause.code
        : undefined
    const refusal =
      Object.values(VaultErrorCode).find((known) => known === code) ??
      (changeDispatched ? VaultErrorCode.VaultChangeNotConfirmed : undefined)
    super(refusal ? `Vault operation refused (${refusal})` : "Vault unavailable")
    this.name = "NessaVaultError"
    this.refusal = refusal
  }
}
