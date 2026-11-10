/**
 * The computers a channel may name, for its settings and for the flow that
 * makes one.
 */
import { useEffect, useState } from "react"
import { useDevicesGateway } from "../../../devices"
import { sortWorkers, workerCard } from "../../../devices/model/devices"

/** One computer a channel may name: its place name and what a person calls it. */
export interface Computer {
  readonly environment: string
  readonly name: string
}

/**
 * The computers a channel may run on now, as Devices lists them: each paired
 * computer that is online, then each SSH host the gateway serves. Read once
 * when the settings open; nothing while there is no devices gateway.
 */
export function useComputers(): readonly Computer[] {
  const gateway = useDevicesGateway()
  const [computers, setComputers] = useState<readonly Computer[]>([])
  useEffect(() => {
    if (!gateway) return
    let current = true
    void gateway.list().then((read) => {
      if (!current || !read.ok) return
      setComputers([
        ...sortWorkers(read.value.workers.map(workerCard))
          .filter((worker) => worker.status === "online")
          .map((worker) => ({ environment: worker.environment, name: worker.name })),
        ...read.value.sshHosts.map((host) => ({
          environment: host.host,
          name: host.host,
        })),
      ])
    })
    return () => {
      current = false
    }
  }, [gateway])
  return computers
}
