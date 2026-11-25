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

describe("devices.list", () => {
  it("asks once and reads the hub and where it listens", async () => {
    const asked: unknown[] = []
    const api = createDevicesApi({
      request: async (method: string, params: unknown) => {
        asked.push([method, params])
        return { hub: machine, listening: "0.0.0.0:47650" }
      },
    } as never)
    expect(await api.list()).toEqual({ hub: machine, listening: "0.0.0.0:47650" })
    expect(asked).toEqual([["devices.list", {}]])
  })

  it("reads a hub it could not read as nothing", () => {
    expect(devicesList({})).toEqual({})
  })

  it("refuses an answer off its schema, workers and SSH hosts included", () => {
    const bad = [
      null,
      { omitted: 0 },
      { workers: [] },
      { sshHosts: [] },
      { defaultEnvironment: "devbox" },
      { next: `peer:${"ab".repeat(32)}` },
      { listening: 47650 },
      { listening: "x".repeat(65) },
      { hub: { ...machine, cpuPermille: 1001 } },
      { hub: { ...machine, memoryUsedBytes: 33 } },
      { hub: { ...machine, hostname: "x".repeat(129) } },
      { hub: { ...machine, gpu: "yes" } },
      { hub: { ...machine, disks: Array(17).fill(machine.disks[0]) } },
      { hub: { ...machine, disks: [{ mount: "/", totalBytes: 1, usedBytes: 2 }] } },
      {
        hub: {
          ...machine,
          disks: [{ mount: "x".repeat(129), totalBytes: 1, usedBytes: 0 }],
        },
      },
      { hub: { ...machine, disks: [{ mount: "/", totalBytes: 1 }] } },
    ]
    for (const value of bad)
      expect(() => devicesList(value), JSON.stringify(value)).toThrow()
  })
})
