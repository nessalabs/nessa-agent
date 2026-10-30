/**
 * The hub's devices, for the Devices view: `client.devices`, `client.boat`,
 * the linked devices and pairing a worker (`pairing.create` with enrollee
 * `environment`), read into the view's words. Who may pair, and whether a
 * key is the one claimed, is the gateway's to say; nothing here decides it.
 */
import {
  PairingEnrollee,
  type AuthApi,
  type BoatApi,
  type CredentialApi,
  type DevicesApi,
  type DeviceMachine,
  type DeviceWorker,
  type PairingApi,
} from "@nessa/client"
import {
  DEVICE_KEY_BYTES,
  INVITATION_BYTES,
  bytesToHex,
  deviceKeyFingerprint,
  hexToBytes,
} from "../../settings/model/device-key"
import { linkedDeviceOf } from "../../adapters/linked-device"
import type { LinkedDevice } from "../../model/linked-device"

/** What the Devices view asks of the window's gateway client. */
export interface DevicesClient {
  readonly devices: DevicesApi
  readonly pairing: PairingApi
  readonly boat: BoatApi
  readonly credentials: CredentialApi
  readonly auth: AuthApi
}

/** An answer, or that none came that this window can read. */
export type Outcome<T> = { readonly ok: true; readonly value: T } | { readonly ok: false }

/** Where a worker's pairing stands. */
export interface WorkerPairing {
  readonly invitationId: string
  readonly phase: "available" | "claimed" | "approved" | "staging" | "active" | "terminal"
  readonly expiresAtMs: number
  /** The claimed key, hex, which approval names. */
  readonly deviceKey?: string
  /** SHA-256 of the claimed key, hex: compared with what the worker prints. */
  readonly fingerprint?: string
}

/** A code made for a worker to enroll with, and its pairing. */
export interface WorkerCode {
  readonly code: string
  readonly pairing: WorkerPairing
}

/**
 * Every page of `devices.list`, read one after another: the hub as the
 * first page read it, and each worker once, in the order of their keys.
 */
export interface DevicesRead {
  readonly hub?: DeviceMachine
  /** Where the hub's native listener is bound; absent while linking is off. */
  readonly listening?: string
  readonly workers: readonly DeviceWorker[]
  /** More pages than [`MAX_PAGES`] follow: workers past these are not read. */
  readonly more: boolean
}

/**
 * The most pages one read follows: 64 workers each, so a fleet of tens of
 * thousands is read whole, and a hub that never ends its list still lets
 * the read end.
 */
export const MAX_PAGES = 1_024

/** Whether a cloud (Boat) connection is saved. */
export type CloudState = "connected" | "disconnected" | "locked"

export interface DevicesGateway {
  /** How often an open view reads again. */
  readonly pollMs: number
  list(): Promise<Outcome<DevicesRead>>
  /**
   * Let other computers reach the hub: turn native linking on, which also
   * moves a listener Settings once bound to this machine alone to every
   * interface. `"restarting"`: the gateway starts again with it, and this
   * connection closes.
   */
  reach(): Promise<Outcome<"restarting" | "unchanged">>
  cloud(): Promise<Outcome<CloudState>>
  /** Phones and apps linked to the hub (Settings › Linked devices). */
  linked(): Promise<Outcome<readonly LinkedDevice[]>>
  /** A pairing code for a computer to enroll as a worker. */
  pair(): Promise<Outcome<WorkerCode>>
  status(invitationId: string): Promise<Outcome<WorkerPairing>>
  approve(invitationId: string, deviceKey: string): Promise<Outcome<WorkerPairing>>
  cancel(invitationId: string): Promise<Outcome<WorkerPairing>>
}

type OwnerStatus = Awaited<ReturnType<PairingApi["status"]>>

async function pairingOf(status: OwnerStatus): Promise<WorkerPairing> {
  const claimed = status.claimedDeviceKey
  const fingerprint = claimed
    ? await deviceKeyFingerprint(Uint8Array.from(claimed)).catch(() => undefined)
    : undefined
  return {
    invitationId: bytesToHex(status.invitationId),
    phase: status.phase,
    expiresAtMs: status.expiresAtMs,
    ...(claimed ? { deviceKey: bytesToHex(claimed) } : {}),
    ...(fingerprint ? { fingerprint } : {}),
  }
}

function bytesOf(hex: string, length: number): number[] {
  const bytes = hexToBytes(hex)
  if (!bytes || bytes.length !== length) throw new Error("not an id")
  return bytes
}

export function devicesGateway(options: {
  readonly connected: () => Promise<DevicesClient>
  readonly pollMs?: number
}): DevicesGateway {
  async function answer<T>(
    run: (client: DevicesClient) => Promise<T>,
  ): Promise<Outcome<T>> {
    try {
      return { ok: true, value: await run(await options.connected()) }
    } catch {
      return { ok: false }
    }
  }
  return {
    pollMs: options.pollMs ?? 5_000,
    list: () =>
      answer(async (client) => {
        const first = await client.devices.list()
        const workers = [...first.workers]
        let next = first.next
        for (let pages = 1; next !== undefined && pages < MAX_PAGES; pages += 1) {
          const page = await client.devices.list(next)
          workers.push(...page.workers)
          next = page.next
        }
        return {
          ...(first.hub ? { hub: first.hub } : {}),
          ...(first.listening !== undefined ? { listening: first.listening } : {}),
          workers,
          more: next !== undefined,
        }
      }),
    reach: () =>
      answer(async (client) => (await client.pairing.setLinking(true)).applies),
    cloud: () => answer(async (client) => (await client.boat.status()).state),
    linked: () =>
      answer(async (client) => {
        const [session, listed] = await Promise.all([
          client.auth.session(),
          client.credentials.list(),
        ])
        return listed.credentials.flatMap((credential) => {
          const device = linkedDeviceOf(credential, session.credentialId)
          return device ? [device] : []
        })
      }),
    pair: () =>
      answer(async (client) => {
        const created = await client.pairing.create({
          enrollee: PairingEnrollee.Environment,
        })
        return { code: created.code, pairing: await pairingOf(created.status) }
      }),
    status: (invitationId) =>
      answer(async (client) =>
        pairingOf(await client.pairing.status(bytesOf(invitationId, INVITATION_BYTES))),
      ),
    approve: (invitationId, deviceKey) =>
      answer(async (client) => {
        const approved = await client.pairing.approve(
          bytesOf(invitationId, INVITATION_BYTES),
          bytesOf(deviceKey, DEVICE_KEY_BYTES),
        )
        return pairingOf(approved.status)
      }),
    cancel: (invitationId) =>
      answer(async (client) =>
        pairingOf(await client.pairing.cancel(bytesOf(invitationId, INVITATION_BYTES))),
      ),
  }
}
