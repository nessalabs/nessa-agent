/**
 * The gateway's linked devices, for Settings: `client.pairing`,
 * `client.credentials` and `client.auth` read into the model's words
 * (`model/linked-devices.ts`). Who may pair is the gateway's to say, by its
 * answers; nothing here reads the credential's grants to decide that. Each
 * pairing refusal is mapped by a total table, so a code the protocol adds is
 * a type error here rather than a sentence nobody wrote.
 *
 * A linked device is a non-revoked credential that is not this session's and
 * whose only grant is `conversation.read`. The list carries no device
 * marker. If the session cannot be read, no device is listed: the session
 * credential must not be offered for revoke.
 */
import {
  NessaMutationError,
  NessaPairingError,
  PairingRefusalCode,
  type AuthApi,
  type ConnectionState,
  type CredentialApi,
  type CredentialMetadata,
  type PairingApi,
  type PairingRefusalCode as PairingRefusal,
} from "@nessa/client"
import {
  DEVICE_KEY_BYTES,
  INVITATION_BYTES,
  bytesToHex,
  deviceKeyFingerprint,
  hexToBytes,
} from "../model/device-key"
import type {
  Approval,
  CreatedCode,
  Enrollment,
  EnrollmentPhase,
  Failure,
  FailureKind,
  LinkedDevice,
  Outcome,
  ReadValue,
  TerminalCause,
} from "../model/linked-devices"

type OwnerStatus = Awaited<ReturnType<PairingApi["status"]>>

/** What Settings asks of the window's gateway client. */
export interface LinkedDevicesClient {
  readonly pairing: PairingApi
  readonly credentials: CredentialApi
  readonly auth: AuthApi
  readonly connectionState: ConnectionState
  onConnectionStateChange(handler: (state: ConnectionState) => void): () => void
}

/** How the connection stands, as the tab follows it. */
export type LinkedDevicesConnection =
  { readonly type: "connected" } | { readonly type: "unreachable" }

/** Pairing and linked devices, as Settings manages them. */
export interface LinkedDevicesGateway {
  /** How often a quiet tab reads again. `0` does not poll. */
  readonly pollMs: number
  follow(handler: (connection: LinkedDevicesConnection) => void): () => void
  read(): Promise<Outcome<ReadValue>>
  create(): Promise<Outcome<CreatedCode>>
  cancel(invitationId: string): Promise<Outcome<Enrollment>>
  /** Cancel, and only then create. A failed cancel does not create. */
  refresh(invitationId: string): Promise<Outcome<CreatedCode>>
  approve(invitationId: string, deviceKey: string): Promise<Outcome<Approval>>
  deny(invitationId: string): Promise<Outcome<Enrollment>>
  status(invitationId: string): Promise<Outcome<Enrollment>>
  revoke(credentialId: string): Promise<Outcome<{ credentialId: string }>>
}

/** The pairing refusals, each one sentence in the model. */
const pairingFailure: Record<PairingRefusal, FailureKind> = {
  [PairingRefusalCode.PairingNotConfigured]: "notConfigured",
  [PairingRefusalCode.PairingNotFound]: "notFound",
  [PairingRefusalCode.PairingSlotOccupied]: "slotOccupied",
  [PairingRefusalCode.PairingCapacity]: "capacity",
  [PairingRefusalCode.PairingConflict]: "conflict",
  [PairingRefusalCode.PairingIneligible]: "ineligible",
  [PairingRefusalCode.PairingBusy]: "busy",
  [PairingRefusalCode.PairingUnavailable]: "unavailable",
  [PairingRefusalCode.Forbidden]: "refused",
  [PairingRefusalCode.Unauthorized]: "unauthorized",
  [PairingRefusalCode.InvalidRequest]: "invalid",
  [PairingRefusalCode.TemporarilyUnavailable]: "temporarilyUnavailable",
}

/** Credential admin codes these methods answer with, beside the session codes. */
const credentialFailure = {
  forbidden: "refused",
  unauthorized: "unauthorized",
  invalid_request: "invalid",
  temporarily_unavailable: "temporarilyUnavailable",
  credential_not_found: "credentialMissing",
  credential_conflict: "conflict",
  credential_capacity: "capacity",
} as const satisfies Record<string, FailureKind>

/** Total over the protocol's phases, so one it adds does not pass as a phase the window has no row for. */
function phaseOf(phase: OwnerStatus["phase"]): EnrollmentPhase {
  switch (phase) {
    case "available":
    case "claimed":
    case "approved":
    case "staging":
    case "active":
    case "terminal":
      return phase
    default: {
      const unexpected: never = phase
      return unexpected
    }
  }
}

/** Total over the protocol's terminal causes. */
function terminalOf(cause: NonNullable<OwnerStatus["terminal"]>["cause"]): TerminalCause {
  switch (cause) {
    case "expired":
      return "expired"
    case "cancelled":
      return "cancelled"
    case "denied":
      return "denied"
    case "credential_revoked":
      return "revoked"
    case "restarted":
      return "restarted"
    default: {
      const unexpected: never = cause
      return unexpected
    }
  }
}

/** What a failed call means to the tab. A code this build does not know is unanswered. */
export function failureOf(error: unknown): Failure {
  if (error instanceof NessaPairingError)
    return { kind: error.refusal ? pairingFailure[error.refusal] : "unanswered" }
  if (error instanceof NessaMutationError) {
    const code = error.code
    if (typeof code === "string" && Object.hasOwn(credentialFailure, code))
      return { kind: credentialFailure[code as keyof typeof credentialFailure] }
  }
  return { kind: "unanswered" }
}

const unanswered = <T>(): Outcome<T> => ({ ok: false, failure: { kind: "unanswered" } })

function bytesOf(hex: string, length: number): number[] | undefined {
  const bytes = hexToBytes(hex)
  return bytes && bytes.length === length ? bytes : undefined
}

async function enrollmentOf(status: OwnerStatus): Promise<Enrollment> {
  const claimed = status.claimedDeviceKey
  const fingerprint = claimed
    ? await deviceKeyFingerprint(Uint8Array.from(claimed)).catch(() => undefined)
    : undefined
  return {
    invitationId: bytesToHex(status.invitationId),
    phase: phaseOf(status.phase),
    expiresAtMs: status.expiresAtMs,
    ...(claimed ? { deviceKey: bytesToHex(claimed) } : {}),
    ...(fingerprint ? { fingerprint } : {}),
    ...(status.credentialId ? { credentialId: status.credentialId } : {}),
    ...(status.terminal ? { terminal: terminalOf(status.terminal.cause) } : {}),
  }
}

/** A device credential, as far as the list can say. The session's own is never one. */
function linkedDevice(
  credential: CredentialMetadata,
  sessionId: string,
): LinkedDevice | undefined {
  if (credential.revokedAt !== null || credential.id === sessionId) return undefined
  if (
    credential.grants.length !== 1 ||
    credential.grants[0]?.action !== "conversation.read"
  )
    return undefined
  return { credentialId: credential.id, issuedAt: credential.issuedAt }
}

async function outcomeOf<T>(run: () => Promise<T>): Promise<Outcome<T>> {
  try {
    return { ok: true, value: await run() }
  } catch (error) {
    return { ok: false, failure: failureOf(error) }
  }
}

/**
 * Reads linking, what is still unfinished, and the devices. Native linking
 * off and a refused owner are reads that succeeded: they are the state.
 * A session that cannot be read lists no device.
 */
async function readAll(client: LinkedDevicesClient): Promise<Outcome<ReadValue>> {
  let pending: Awaited<ReturnType<PairingApi["pending"]>>
  try {
    pending = await client.pairing.pending()
  } catch (error) {
    const failure = failureOf(error)
    if (failure.kind === "notConfigured") return { ok: true, value: { kind: "off" } }
    if (failure.kind === "refused") return { ok: true, value: { kind: "refused" } }
    return { ok: false, failure }
  }
  let sessionId: string
  let credentials: CredentialMetadata[]
  try {
    const [session, listed] = await Promise.all([
      client.auth.session(),
      client.credentials.list(),
    ])
    sessionId = session.credentialId
    credentials = listed.credentials
  } catch (error) {
    return { ok: false, failure: failureOf(error) }
  }
  const enrollments = await Promise.all(pending.items.map(enrollmentOf))
  const devices = credentials.flatMap((credential) => {
    const device = linkedDevice(credential, sessionId)
    return device ? [device] : []
  })
  return { ok: true, value: { kind: "on", enrollments, devices } }
}

export function linkedDevicesGateway(options: {
  readonly connected: () => Promise<LinkedDevicesClient>
  readonly after: (ms: number, run: () => void) => () => void
  readonly retryMs?: number
  readonly pollMs?: number
}): LinkedDevicesGateway {
  const { connected, after } = options
  const retryMs = options.retryMs ?? 2_000
  const client = () => connected()
  return {
    pollMs: options.pollMs ?? 4_000,
    follow(handler) {
      let stopped = false
      let off = () => {}
      let cancelRetry = () => {}
      const retry = () => {
        cancelRetry = after(retryMs, attempt)
      }
      function attempt() {
        connected().then(
          (current) => {
            if (stopped) return
            const tell = (connection: ConnectionState) => {
              if (stopped) return
              if (connection.status === "connected") {
                handler({ type: "connected" })
                return
              }
              handler({ type: "unreachable" })
              if (connection.status === "closed") {
                off()
                retry()
              }
            }
            off = current.onConnectionStateChange(tell)
            tell(current.connectionState)
          },
          () => {
            if (stopped) return
            handler({ type: "unreachable" })
            retry()
          },
        )
      }
      attempt()
      return () => {
        stopped = true
        off()
        cancelRetry()
      }
    },
    read: () => client().then(readAll, () => unanswered()),
    create: () =>
      client().then(
        (current) =>
          outcomeOf(async () => {
            const created = await current.pairing.create()
            return { code: created.code, enrollment: await enrollmentOf(created.status) }
          }),
        () => unanswered(),
      ),
    cancel: (invitationId) =>
      client().then(
        (current) =>
          outcomeOf(async () => {
            const id = bytesOf(invitationId, INVITATION_BYTES)
            if (!id) throw new Error("invitation id")
            return enrollmentOf(await current.pairing.cancel(id))
          }),
        () => unanswered(),
      ),
    async refresh(invitationId) {
      const current = await client().catch(() => undefined)
      if (!current) return unanswered()
      const id = bytesOf(invitationId, INVITATION_BYTES)
      if (!id) return unanswered()
      try {
        await current.pairing.cancel(id)
      } catch (error) {
        return { ok: false, failure: failureOf(error) }
      }
      try {
        const created = await current.pairing.create()
        return {
          ok: true,
          value: { code: created.code, enrollment: await enrollmentOf(created.status) },
        }
      } catch (error) {
        return { ok: false, failure: failureOf(error), cancelled: true }
      }
    },
    approve: (invitationId, deviceKey) =>
      client().then(
        (current) =>
          outcomeOf(async () => {
            const id = bytesOf(invitationId, INVITATION_BYTES)
            const key = bytesOf(deviceKey, DEVICE_KEY_BYTES)
            if (!id || !key) throw new Error("approval id")
            const approved = await current.pairing.approve(id, key)
            return {
              enrollment: await enrollmentOf(approved.status),
              ...(approved.activationStopped
                ? { activationStopped: approved.activationStopped }
                : {}),
            }
          }),
        () => unanswered(),
      ),
    deny: (invitationId) =>
      client().then(
        (current) =>
          outcomeOf(async () => {
            const id = bytesOf(invitationId, INVITATION_BYTES)
            if (!id) throw new Error("invitation id")
            return enrollmentOf(await current.pairing.deny(id))
          }),
        () => unanswered(),
      ),
    status: (invitationId) =>
      client().then(
        (current) =>
          outcomeOf(async () => {
            const id = bytesOf(invitationId, INVITATION_BYTES)
            if (!id) throw new Error("invitation id")
            return enrollmentOf(await current.pairing.status(id))
          }),
        () => unanswered(),
      ),
    revoke: (credentialId) =>
      client().then(
        (current) =>
          outcomeOf(async () => {
            const revoked = await current.credentials.revoke(credentialId)
            return { credentialId: revoked.credentialId }
          }),
        () => unanswered(),
      ),
  }
}
