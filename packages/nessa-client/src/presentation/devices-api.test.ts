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
}
const environment = `peer:${"ab".repeat(32)}`

describe("devices.list", () => {
  it("asks devices.list with no params and reads the hub and its workers", async () => {
    const asked: unknown[] = []
    const api = createDevicesApi({
      request: async (method: string, params: unknown) => {
        asked.push([method, params])
        return {
          hub: machine,
          workers: [
            { environment, state: "online", agents: 3, machine, lastSeenMs: 5 },
            { environment, state: "offline", agents: 0 },
          ],
        }
      },
    } as never)
    expect(await api.list()).toEqual({
      hub: machine,
      workers: [
        { environment, state: "online", agents: 3, machine, lastSeenMs: 5 },
        { environment, state: "offline", agents: 0 },
      ],
    })
    expect(asked).toEqual([["devices.list", {}]])
  })

  it("refuses an answer off its schema", () => {
    const bad = [
      null,
      { workers: [], extra: 1 },
      { workers: [{ environment: "peer:xyz", state: "online", agents: 0 }] },
      { workers: [{ environment, state: "asleep", agents: 0 }] },
      { workers: [{ environment, state: "online", agents: -1 }] },
      { workers: [], hub: { ...machine, cpuPermille: 1001 } },
      { workers: [], hub: { ...machine, memoryUsedBytes: 33 } },
      { workers: [], hub: { ...machine, hostname: "x".repeat(129) } },
      { workers: [], hub: { ...machine, gpu: "yes" } },
    ]
    for (const value of bad)
      expect(() => devicesList(value), JSON.stringify(value)).toThrow()
  })
})
