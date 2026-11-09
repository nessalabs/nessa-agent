import { vaultCommand } from "../../host"
import {
  VaultRefusal,
  type VaultSource,
  type VaultFailure,
  type VaultState,
} from "../application/ports"
import {
  VaultErrorCode,
  VaultState as States,
  VaultPassphraseChangeState,
} from "@nessa/client"
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
  changePassphrase: (passphrase) => call("changePassphrase", passphrase),
  lock: () => call("lock"),
}
async function call(
  operation: "status" | "setup" | "unlock" | "lock" | "changePassphrase",
  passphrase?: string,
): Promise<VaultState> {
  let raw: unknown
  const unknown =
    operation === "changePassphrase" ? "vault_change_not_confirmed" : "vault_unavailable"
  try {
    raw = await vaultCommand({
      operation,
      ...(passphrase === undefined ? {} : { passphrase }),
    })
  } catch (cause) {
    // Only the native command's closed serialized refusal codes are trusted;
    // arbitrary errors can follow delivery and must remain unconfirmed.
    switch (cause) {
      case "vault_busy":
        throw new VaultRefusal("vault_busy")
      case "forbidden":
        throw new VaultRefusal("vault_forbidden")
      case "vault_unavailable":
        throw new VaultRefusal("vault_unavailable")
      case "vault_change_not_confirmed":
        throw new VaultRefusal("vault_change_not_confirmed")
      default:
        throw new VaultRefusal(unknown)
    }
  }
  if (typeof raw !== "object" || raw === null || Array.isArray(raw))
    throw new VaultRefusal(unknown)
  const value = raw as Record<string, unknown>
  if (
    Object.hasOwn(value, "ok") &&
    value.ok === true &&
    Object.keys(value).length === 2 &&
    Object.hasOwn(value, "status") &&
    typeof value.status === "object" &&
    value.status !== null
  ) {
    const status = value.status as Record<string, unknown>
    const state = Object.hasOwn(status, "state") ? status.state : undefined
    if (
      !Array.isArray(status) &&
      Object.keys(status).length === 1 &&
      Object.values(States).some((known) => known === state) &&
      (operation !== "changePassphrase" || state === VaultPassphraseChangeState.Locked)
    )
      return state as VaultState
  }
  const code = Object.hasOwn(value, "code") ? value.code : undefined
  if (
    !Object.hasOwn(value, "ok") ||
    value.ok !== false ||
    Object.keys(value).length !== 2
  )
    throw new VaultRefusal(unknown)
  throw new VaultRefusal(
    Object.values(VaultErrorCode).some((known) => known === code)
      ? (code as VaultFailure)
      : unknown,
  )
}
