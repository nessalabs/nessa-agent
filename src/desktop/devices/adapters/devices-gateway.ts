/**
 * The hub's devices, for the Devices view: `client.devices`, `client.boat`
 * and the linked devices, read into the view's words. Every gateway runs
 * only its own agents (ADR 1417): another computer runs a gateway of its
 * own, so there are no workers here to pair or list.
 */
import {
  type AuthApi,
  type BoatApi,
  type CredentialApi,
  type DevicesApi,
  type DeviceMachine,
} from "@nessa/client"
import { linkedDeviceOf } from "../../adapters/linked-device"
import type { LinkedDevice } from "../../model/linked-device"

/** What the Devices view asks of the window's gateway client. */
export interface DevicesClient {
  readonly devices: DevicesApi
  readonly boat: BoatApi
  readonly credentials: CredentialApi
  readonly auth: AuthApi
}

/** An answer, or that none came that this window can read. */
export type Outcome<T> = { readonly ok: true; readonly value: T } | { readonly ok: false }

/** What `devices.list` said of the hub. */
export interface DevicesRead {
  readonly hub?: DeviceMachine
}

/** Whether a cloud (Boat) connection is saved. */
export type CloudState = "connected" | "disconnected" | "locked"

export interface DevicesGateway {
  /** How often an open view reads again. */
  readonly pollMs: number
  list(): Promise<Outcome<DevicesRead>>
  cloud(): Promise<Outcome<CloudState>>
  /** Phones and apps linked to the hub (Settings › Linked devices). */
  linked(): Promise<Outcome<readonly LinkedDevice[]>>
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
        const listed = await client.devices.list()
        return listed.hub ? { hub: listed.hub } : {}
      }),
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
  }
}
