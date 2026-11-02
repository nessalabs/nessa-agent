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
      {
        workers: [],
        hub: { ...machine, disks: [{ mount: "/", totalBytes: 1, usedBytes: 2 }] },
      },
      {
        workers: [],
        hub: {
          ...machine,
          disks: [{ mount: "x".repeat(129), totalBytes: 1, usedBytes: 0 }],
        },
      },
      { workers: [], hub: { ...machine, disks: [{ mount: "/", totalBytes: 1 }] } },
    ]
    for (const value of bad)
      expect(() => devicesList(value), JSON.stringify(value)).toThrow()
  })
})

describe("devices SSH hosts and the default device", () => {
  const answering = (answer: unknown, asked: unknown[] = []) =>
    createDevicesApi({
      request: async (method: string, params: unknown) => {
        asked.push([method, params])
        return answer
      },
    } as never)

  it("reads the configured hosts and the default on the list's first page", () => {
    expect(
      devicesList({
        workers: [],
        sshHosts: [{ host: "box", hostName: "10.0.0.5", user: "ada", port: 22 }],
        defaultEnvironment: "box",
      }),
    ).toEqual({
      workers: [],
      sshHosts: [{ host: "box", hostName: "10.0.0.5", user: "ada", port: 22 }],
      defaultEnvironment: "box",
    })
    for (const bad of [
      { workers: [], defaultEnvironment: "" },
      { workers: [], defaultEnvironment: null },
      { workers: [], sshHosts: [{ host: "box", port: 0 }] },
      { workers: [], sshHosts: [{ host: "box", alias: "x" }] },
      { workers: [], sshHosts: Array(17).fill({ host: "box" }) },
    ])
      expect(() => devicesList(bad), JSON.stringify(bad)).toThrow(TypeError)
  })

  it("reads suggestions and connections, a failed one with its refusal", async () => {
    const asked: unknown[] = []
    const answer = {
      suggested: [{ host: "box", user: "ada" }],
      connecting: [
        { host: "build", state: "installing" },
        { host: "old", state: "failed", refusal: "environment_unreachable" },
      ],
    }
    expect(await answering(answer, asked).sshHosts()).toEqual(answer)
    expect(asked).toEqual([["devices.sshHosts", {}]])
    for (const connecting of [
      [{ host: "build", state: "failed" }],
      [{ host: "build", state: "installing", refusal: "devices_busy" }],
      [{ host: "build", state: "failed", refusal: "nope" }],
      [{ host: "build", state: "waiting" }],
    ])
      await expect(
        answering({ suggested: [], connecting }).sshHosts(),
        JSON.stringify(connecting),
      ).rejects.toThrow(TypeError)
  })

  it("connects a host and saves the default", async () => {
    const asked: unknown[] = []
    expect(
      await answering(
        { connecting: { host: "box", state: "installing" } },
        asked,
      ).connectSsh("box"),
    ).toEqual({ host: "box", state: "installing" })
    expect(
      await answering({ defaultEnvironment: null }, asked).setDefault(null),
    ).toBeNull()
    expect(await answering({ defaultEnvironment: "box" }, asked).setDefault("box")).toBe(
      "box",
    )
    expect(asked).toEqual([
      ["devices.connectSsh", { host: "box" }],
      ["devices.setDefault", { environment: null }],
      ["devices.setDefault", { environment: "box" }],
    ])
    await expect(answering({}).setDefault(null)).rejects.toThrow(TypeError)
  })
})
