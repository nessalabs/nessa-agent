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
          listening: "0.0.0.0:47650",
          workers: [
            { environment, state: "online", agents: 3, machine, lastSeenMs: 5 },
            { environment, state: "offline", agents: 0 },
          ],
          omitted: 2,
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
      omitted: 2,
    })
    expect(asked).toEqual([["devices.list", {}]])
  })

  it("refuses an answer off its schema", () => {
    const bad = [
      null,
      { workers: [] },
      { workers: [], omitted: -1 },
      { workers: [], omitted: 0, listening: 47650 },
      { workers: [], omitted: 0, listening: "x".repeat(65) },
      {
        workers: Array(65).fill({ environment, state: "offline", agents: 0 }),
        omitted: 0,
      },
      { workers: [{ environment: "peer:xyz", state: "online", agents: 0 }], omitted: 0 },
      { workers: [{ environment, state: "asleep", agents: 0 }], omitted: 0 },
      { workers: [{ environment, state: "online", agents: -1 }], omitted: 0 },
      { workers: [], omitted: 0, hub: { ...machine, cpuPermille: 1001 } },
      { workers: [], omitted: 0, hub: { ...machine, memoryUsedBytes: 33 } },
      { workers: [], omitted: 0, hub: { ...machine, hostname: "x".repeat(129) } },
      { workers: [], omitted: 0, hub: { ...machine, gpu: "yes" } },
    ]
    for (const value of bad)
      expect(() => devicesList(value), JSON.stringify(value)).toThrow()
  })
})
