/**
 * Where each cloud's connection stands, read from its own gateway. Each
 * cloud in `clouds.ts` has its reader here; a cloud with none fails the
 * build.
 */
import { useEffect, useState } from "react"
import type { Cloud as GatewayCloud } from "@nessa/client"
import type { CloudState } from "../../devices/adapters/devices-gateway"
import { useAgent37Gateway } from "../../settings/ui/agent37-row"
import { useBoatGateway } from "../../settings/ui/boat-row"
import type { Cloud } from "../model/clouds"

// The registry's clouds are the gateway's, no more and no fewer.
type Same<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false
const registryMatchesGateway: Same<Cloud, GatewayCloud> = true
void registryMatchesGateway

/**
 * Each cloud's state: `null` when it could not be read, `undefined` until
 * read, or for good when this window has no gateway for that cloud.
 */
export type CloudStates = { readonly [id in Cloud]?: CloudState | null }

type Reader = { readonly read: () => Promise<{ readonly state: CloudState }> } | undefined

/**
 * Reads every cloud's state now, and again whenever `seen` changes (what
 * another read last said, or a counter a save bumps).
 */
export function useCloudStates(seen?: unknown): CloudStates {
  const readers: { readonly [id in Cloud]: Reader } = {
    boat: useBoatGateway(),
    agent37: useAgent37Gateway(),
  }
  const [states, setStates] = useState<CloudStates>({})
  const { boat, agent37 } = readers
  useEffect(() => {
    let live = true
    for (const [id, reader] of Object.entries({ boat, agent37 }) as [Cloud, Reader][]) {
      // A cloud this window has no gateway for is not offered at all.
      if (!reader) continue
      reader
        .read()
        .then(
          (status) => status.state,
          () => null,
        )
        .then((state) => {
          if (live) setStates((last) => ({ ...last, [id]: state }))
        })
    }
    return () => {
      live = false
    }
  }, [boat, agent37, seen])
  return states
}
