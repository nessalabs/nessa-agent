import type { VaultState } from "@nessa/client"
/** Shared local-vault UI seam. Implementations retain the appropriate owner
 * connection, without exposing a key or any stored credential to the UI. */
export interface VaultSource {
  status(): Promise<VaultState>
  /** `remember` keeps the accepted passphrase for Touch ID, where offered. */
  setup(passphrase: string, remember?: boolean): Promise<VaultState>
  unlock(passphrase: string, remember?: boolean): Promise<VaultState>
  changePassphrase(passphrase: string): Promise<VaultState>
  lock(): Promise<VaultState>
  /** Touch ID, only where the native host can offer it. */
  readonly biometrics?: VaultBiometrics
}
/** Whether this device checks a fingerprint, and whether a passphrase is kept for it. */
export interface BiometricStatus {
  readonly available: boolean
  readonly saved: boolean
}
/** The host keeps the passphrase; the window only asks it to unlock. */
export interface VaultBiometrics {
  status(): Promise<BiometricStatus>
  /** Shows the system's Touch ID prompt, then unlocks with the kept passphrase. */
  unlock(): Promise<VaultState>
  forget(): Promise<BiometricStatus>
}
export type { VaultState } from "@nessa/client"
/** Why Touch ID did not unlock; the passphrase form is the way in after each. */
export const biometricFailures = [
  "biometric_unavailable",
  "biometric_cancelled",
  "biometric_changed",
  "biometric_missing",
] as const
export type BiometricFailure = (typeof biometricFailures)[number]
export type VaultFailure = import("@nessa/client").VaultErrorCode | BiometricFailure
/** Only a known redacted code crosses the UI port. */
export class VaultRefusal extends Error {
  constructor(readonly code: VaultFailure) {
    super(code)
    this.name = "VaultRefusal"
  }
}
