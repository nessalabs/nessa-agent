import type { VaultState } from "@nessa/client"
/** Shared local-vault UI seam. Implementations retain the appropriate owner
 * connection, without exposing a key or any stored credential to the UI. */
export interface VaultSource {
  status(): Promise<VaultState>
  setup(passphrase: string): Promise<VaultState>
  unlock(passphrase: string): Promise<VaultState>
  changePassphrase(passphrase: string): Promise<VaultState>
  lock(): Promise<VaultState>
}
export type { VaultState } from "@nessa/client"
export type VaultFailure = import("@nessa/client").VaultErrorCode
/** Only a known redacted code crosses the UI port. */
export class VaultRefusal extends Error {
  constructor(readonly code: VaultFailure) {
    super(code)
    this.name = "VaultRefusal"
  }
}
