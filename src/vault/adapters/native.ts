import { vaultCommand } from "../../host"
import {
  VaultRefusal,
  type VaultSource,
  type VaultFailure,
  type VaultState,
} from "../application/ports"
import { VaultErrorCode, VaultState as States } from "@nessa/client"
let statusRead: Promise<VaultState> | undefined
/** Native windows share the host's retained child, across onboarding/reloads. */
export const nativeVault: VaultSource = {
  status: () => {
    // React mount replay and focus can ask together. Only this secret-free read
    // is coalesced; setup/unlock/lock are never queued or replayed.
    if (!statusRead)
      statusRead = call("status").finally(() => {
        statusRead = undefined
      })
    return statusRead
  },
  setup: (passphrase) => call("setup", passphrase),
  unlock: (passphrase) => call("unlock", passphrase),
  lock: () => call("lock"),
}
async function call(
  operation: "status" | "setup" | "unlock" | "lock",
  passphrase?: string,
): Promise<VaultState> {
  let raw: unknown
  try {
    raw = await vaultCommand({
      operation,
      ...(passphrase === undefined ? {} : { passphrase }),
    })
  } catch {
    throw new VaultRefusal("vault_unavailable")
  }
  if (typeof raw !== "object" || raw === null || Array.isArray(raw))
    throw new VaultRefusal("vault_unavailable")
  const value = raw as Record<string, unknown>
  if (
    Object.hasOwn(value, "ok") &&
    value.ok === true &&
    Object.hasOwn(value, "status") &&
    typeof value.status === "object" &&
    value.status !== null
  ) {
    const status = value.status as Record<string, unknown>
    const state = Object.hasOwn(status, "state") ? status.state : undefined
    if (Object.values(States).some((known) => known === state)) return state as VaultState
  }
  const code = Object.hasOwn(value, "code") ? value.code : undefined
  throw new VaultRefusal(
    Object.values(VaultErrorCode).some((known) => known === code)
      ? (code as VaultFailure)
      : "vault_unavailable",
  )
}
