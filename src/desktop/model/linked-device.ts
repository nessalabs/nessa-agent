/**
 * A linked device, as far as the gateway's credential list can say: its
 * credential and when it was issued. Which credentials are linked devices is
 * one rule (`../adapters/linked-device.ts`); Settings › Linked devices and the
 * share dialog both read devices in this shape.
 */
export interface LinkedDevice {
  readonly credentialId: string
  /** When it was linked, Unix seconds. */
  readonly issuedAt: number
  /**
   * The owner's phone, which reads and writes every chat as the owner. It
   * needs no share, so the share dialog does not offer it.
   */
  readonly fullAccess: boolean
}

/** The day a device was linked, as both Settings and the share dialog say it: `2025-10-09`. */
export function linkedOn(issuedAt: number): string {
  return new Date(issuedAt * 1000).toISOString().slice(0, 10)
}
