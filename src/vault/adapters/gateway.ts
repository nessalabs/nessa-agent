import { NessaVaultError, type VaultApi } from "@nessa/client"
import { VaultRefusal, type VaultSource } from "../application/ports"
/** Browser development uses its existing authenticated product connection. */
export function gatewayVault(
  connected: () => Promise<{ readonly vault: VaultApi }>,
): VaultSource {
  async function call(operation: keyof VaultApi, passphrase?: string) {
    try {
      const { vault } = await connected()
      const result =
        operation === "setup" ||
        operation === "unlock" ||
        operation === "changePassphrase"
          ? await vault[operation](passphrase ?? "")
          : await vault[operation]()
      return result.state
    } catch (cause) {
      throw new VaultRefusal(
        cause instanceof NessaVaultError && cause.refusal
          ? cause.refusal
          : operation === "changePassphrase"
            ? "vault_change_not_confirmed"
            : "vault_unavailable",
      )
    }
  }
  return {
    status: () => call("status"),
    setup: (p) => call("setup", p),
    unlock: (p) => call("unlock", p),
    changePassphrase: (p) => call("changePassphrase", p),
    lock: () => call("lock"),
  }
}
