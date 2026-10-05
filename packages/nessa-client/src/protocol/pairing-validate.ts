/**
 * The answers of the owner `pairing.*` methods, held to the schema's shape
 * before anyone believes them. Whether an enrollment may take a step is the
 * gateway's rule, answered as a typed refusal; this only checks that an
 * answer is one the schema describes.
 */
import {
  PairingActivationStop,
  PairingInitiatorKind,
  PairingOwnerPhase,
  PairingTerminalCause,
  type PairingApproveResult,
  type PairingCreateResult,
  type PairingOwnerStatus,
  type PairingPendingResult,
} from "../generated/product.js"

const invitationBytes = 16
const consentBytes = 16
const deviceKeyBytes = 32
const codeLength = 9

function invalid(what: string): never {
  throw new Error(`pairing response has invalid ${what}`)
}

function record(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value))
    invalid("shape")
  return value as Record<string, unknown>
}

function own(item: Record<string, unknown>, key: string): unknown {
  return Object.hasOwn(item, key) ? item[key] : undefined
}

function bytes(value: unknown, length: number, what: string): number[] {
  if (!Array.isArray(value) || value.length !== length) invalid(what)
  for (const byte of value) {
    if (!Number.isInteger(byte) || (byte as number) < 0 || (byte as number) > 255)
      invalid(what)
  }
  return value as number[]
}

function member<T extends string>(
  table: Record<string, T>,
  value: unknown,
): T | undefined {
  return typeof value === "string" && (Object.values(table) as string[]).includes(value)
    ? (value as T)
    : undefined
}

function millis(value: unknown, what: string): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) invalid(what)
  return value as number
}

/** One enrollment as `pairing.status`, `pairing.deny` and `pairing.cancel` answer it. */
export function pairingOwnerStatus(value: unknown): PairingOwnerStatus {
  const item = record(value)
  const phase = member(PairingOwnerPhase, own(item, "phase"))
  if (!phase) invalid("phase")
  const grant = record(own(item, "grant"))
  const resource = record(own(grant, "resource"))
  if (typeof own(grant, "action") !== "string" || own(grant, "action") === "")
    invalid("grant")
  if (
    typeof own(resource, "organizationId") !== "string" ||
    own(resource, "organizationId") === "" ||
    typeof own(resource, "id") !== "string" ||
    own(resource, "id") === ""
  )
    invalid("grant")
  const status: PairingOwnerStatus = {
    invitationId: bytes(own(item, "invitationId"), invitationBytes, "invitationId"),
    consentId: bytes(own(item, "consentId"), consentBytes, "consentId"),
    generation: millis(own(item, "generation"), "generation"),
    class:
      typeof own(item, "class") === "string"
        ? (own(item, "class") as string)
        : invalid("class"),
    grant: {
      action: own(grant, "action") as string,
      resource: {
        organizationId: own(resource, "organizationId") as string,
        id: own(resource, "id") as string,
      },
    },
    createdAtMs: millis(own(item, "createdAtMs"), "createdAtMs"),
    expiresAtMs: millis(own(item, "expiresAtMs"), "expiresAtMs"),
    phase,
    cleanupPending:
      typeof own(item, "cleanupPending") === "boolean"
        ? (own(item, "cleanupPending") as boolean)
        : invalid("cleanupPending"),
  }
  const claimed = own(item, "claimedDeviceKey")
  if (claimed !== undefined)
    status.claimedDeviceKey = bytes(claimed, deviceKeyBytes, "claimedDeviceKey")
  const credentialId = own(item, "credentialId")
  if (credentialId !== undefined) {
    if (typeof credentialId !== "string" || credentialId.length === 0)
      invalid("credentialId")
    status.credentialId = credentialId
  }
  const receiver = own(item, "receiver")
  if (receiver !== undefined) {
    const body = record(receiver)
    if (typeof own(body, "receiverId") !== "string" || own(body, "receiverId") === "")
      invalid("receiver")
    status.receiver = {
      receiverId: own(body, "receiverId") as string,
      accessEpoch: millis(own(body, "accessEpoch"), "receiver"),
    }
  }
  const terminal = own(item, "terminal")
  if (terminal !== undefined) {
    const body = record(terminal)
    const cause = member(PairingTerminalCause, own(body, "cause"))
    const initiator = record(own(body, "initiator"))
    const kind = member(PairingInitiatorKind, own(initiator, "kind"))
    if (!cause || !kind) invalid("terminal")
    const principalId = own(initiator, "principalId")
    const deviceKey = own(initiator, "deviceKey")
    status.terminal = {
      cause,
      initiator: {
        kind,
        ...(principalId === undefined
          ? {}
          : typeof principalId === "string"
            ? { principalId }
            : invalid("terminal")),
        ...(deviceKey === undefined
          ? {}
          : { deviceKey: bytes(deviceKey, deviceKeyBytes, "terminal") }),
      },
    }
  }
  return status
}

/** The answer to `pairing.create`: the code, shown once, and the invitation it belongs to. */
export function pairingCreateResult(value: unknown): PairingCreateResult {
  const item = record(value)
  const code = own(item, "code")
  if (typeof code !== "string" || code.length !== codeLength) invalid("code")
  return { code, status: pairingOwnerStatus(own(item, "status")) }
}

/** The answer to `pairing.pending`. */
export function pairingPendingResult(value: unknown): PairingPendingResult {
  const item = record(value)
  const items = own(item, "items")
  if (!Array.isArray(items)) invalid("items")
  return { items: items.map(pairingOwnerStatus) }
}

/** The answer to `pairing.approve`. */
export function pairingApproveResult(value: unknown): PairingApproveResult {
  const item = record(value)
  const stopped = own(item, "activationStopped")
  const activationStopped =
    stopped === undefined ? undefined : member(PairingActivationStop, stopped)
  if (stopped !== undefined && !activationStopped) invalid("activationStopped")
  return {
    status: pairingOwnerStatus(own(item, "status")),
    ...(activationStopped ? { activationStopped } : {}),
  }
}
