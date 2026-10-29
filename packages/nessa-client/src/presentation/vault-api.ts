import type { RpcRequester } from "../application/session-port.js"
import { NessaVaultError } from "../application/vault-error.js"
import { ProductMethod, VaultState, type VaultStatus } from "../generated/product.js"
/** Local vault controls. The connection that unlocks owns the lease. Closing it
 * locks the vault; no method returns credentials or keys. Calls are not replayed.
 * Keep passphrases transient and out of logs, settings and persisted UI state. */
export type VaultApi = {
  /** Read lifecycle state without reading a secret entry. */
  status(): Promise<VaultStatus>
  /** Create encrypted storage with a new passphrase. Never overwrites a vault. */
  setup(passphrase: string): Promise<VaultStatus>
  /** Verify the passphrase and retain unlock until this owner connection closes. */
  unlock(passphrase: string): Promise<VaultStatus>
  /** Clear this connection's retained unlock; does not delete stored data. */
  lock(): Promise<VaultStatus>
}
/** Validate a secret-free lifecycle projection, including its complete shape. */
export function vaultStatus(value: unknown): VaultStatus {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    throw new Error("Invalid vault status")
  const item = value as Record<string, unknown>
  const state = Object.hasOwn(item, "state") ? item.state : undefined
  if (
    Object.keys(item).length !== 1 ||
    !Object.values(VaultState).some((known) => known === state)
  )
    throw new Error("Invalid vault state")
  return { state: state as VaultState }
}
export function createVaultApi(session: RpcRequester): VaultApi {
  async function call(method: string, params: Record<string, unknown> = {}) {
    try {
      return vaultStatus(await session.request(method, params))
    } catch (cause) {
      throw new NessaVaultError(cause)
    }
  }
  return {
    status: () => call(ProductMethod.VaultStatus),
    setup: (passphrase) => call(ProductMethod.VaultSetup, { passphrase }),
    unlock: (passphrase) => call(ProductMethod.VaultUnlock, { passphrase }),
    lock: () => call(ProductMethod.VaultLock),
  }
}
