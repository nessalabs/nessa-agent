import { describe, expect, it } from "vitest"
import { createDevicesApi, devicesList } from "./devices-api.js"

const machine = {
  hostname: "build-box",
  os: "linux",
  osVersion: "24.04",
  arch: "x86_64",
  cpus: 8,
  cpuPermille: 610,
  memoryTotalBytes: 32,
  memoryUsedBytes: 8,
  diskTotalBytes: 100,
  diskUsedBytes: 40,
  disks: [{ mount: "/", totalBytes: 100, usedBytes: 40 }],
}
const environment = `peer:${"ab".repeat(32)}`

describe("devices.list", () => {
  it("asks the first page and reads the hub and its workers", async () => {
    const asked: unknown[] = []
    const api = createDevicesApi({
      request: async (method: string, params: unknown) => {
        asked.push([method, params])
        return {
          hub: machine,
          listening: "0.0.0.0:47650",
          workers: [
            { environment, state: "online", agents: 3, machine, lastSeenMs: 5 },
            { environment, state: "offline", agents: 0 },
          ],
          next: environment,
        }
      },
    } as never)
    expect(await api.list()).toEqual({
      hub: machine,
      listening: "0.0.0.0:47650",
      workers: [
        { environment, state: "online", agents: 3, machine, lastSeenMs: 5 },
        { environment, state: "offline", agents: 0 },
      ],
      next: environment,
    })
    expect(asked).toEqual([["devices.list", {}]])
  })

  it("asks for the page after a worker", async () => {
    const asked: unknown[] = []
    const api = createDevicesApi({
      request: async (method: string, params: unknown) => {
        asked.push([method, params])
        return { workers: [] }
      },
    } as never)
    expect(await api.list(environment)).toEqual({ workers: [] })
    expect(asked).toEqual([["devices.list", { after: environment }]])
  })

  it("refuses an answer off its schema", () => {
    const bad = [
      null,
      { workers: [], omitted: 0 },
      { workers: [], next: "peer:xyz" },
      { workers: [], listening: 47650 },
      { workers: [], listening: "x".repeat(65) },
      { workers: Array(65).fill({ environment, state: "offline", agents: 0 }) },
      { workers: [{ environment: "peer:xyz", state: "online", agents: 0 }] },
      { workers: [{ environment, state: "asleep", agents: 0 }] },
      { workers: [{ environment, state: "online", agents: -1 }] },
      { workers: [], hub: { ...machine, cpuPermille: 1001 } },
      { workers: [], hub: { ...machine, memoryUsedBytes: 33 } },
      { workers: [], hub: { ...machine, hostname: "x".repeat(129) } },
      { workers: [], hub: { ...machine, gpu: "yes" } },
      { workers: [], hub: { ...machine, disks: Array(17).fill(machine.disks[0]) } },
      { workers: [], hub: { ...machine, disks: [{ mount: "/", totalBytes: 1, usedBytes: 2 }] } },
      { workers: [], hub: { ...machine, disks: [{ mount: "x".repeat(129), totalBytes: 1, usedBytes: 0 }] } },
      { workers: [], hub: { ...machine, disks: [{ mount: "/", totalBytes: 1 }] } },
    ]
    for (const value of bad)
      expect(() => devicesList(value), JSON.stringify(value)).toThrow()
  })
})
