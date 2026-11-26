import { describe, expect, it } from "vitest"
import { devicesGateway, type DevicesClient } from "./devices-gateway"

const machine = {
  hostname: "studio",
  os: "macos",
  osVersion: "26.1",
  arch: "aarch64",
  cpus: 8,
  cpuPermille: 100,
  memoryTotalBytes: 2,
  memoryUsedBytes: 1,
  diskTotalBytes: 2,
  diskUsedBytes: 1,
}

describe("devicesGateway list", () => {
  it("reads the hub once: there are no workers to page through", async () => {
    const asked: unknown[] = []
    const devices = {
      list: async (...args: unknown[]) => {
        asked.push(args)
        return { hub: machine, listening: "0.0.0.0:47650" }
      },
    }
    const connected = { devices } as unknown as DevicesClient
    const read = await devicesGateway({ connected: async () => connected }).list()
    expect(read).toEqual({ ok: true, value: { hub: machine } })
    expect(asked).toEqual([[]])
  })

  it("reads a hub it could not read as none, and a failed read as unanswered", async () => {
    const empty = { devices: { list: async () => ({}) } } as unknown as DevicesClient
    expect(await devicesGateway({ connected: async () => empty }).list()).toEqual({
      ok: true,
      value: {},
    })
    const failing = {
      devices: {
        list: async () => {
          throw new Error("down")
        },
      },
    } as unknown as DevicesClient
    expect(await devicesGateway({ connected: async () => failing }).list()).toEqual({
      ok: false,
    })
  })
})
