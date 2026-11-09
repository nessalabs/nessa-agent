import { describe, expect, it } from "vitest"
import { MAX_PAGES, devicesGateway, type DevicesClient } from "./devices-gateway"

const worker = (index: number) => ({
  environment: `peer:${index.toString(16).padStart(64, "0")}`,
  state: "offline" as const,
  agents: 0,
})

function client(pages: number, endless = false) {
  const asked: (string | undefined)[] = []
  const devices = {
    list: async (after?: string) => {
      asked.push(after)
      const page = after === undefined ? 0 : Number.parseInt(after.slice(5), 16)
      const last = !endless && page + 1 >= pages
      return {
        ...(page === 0 ? { hub: undefined, listening: "0.0.0.0:47650" } : {}),
        workers: [worker(page + 1)],
        ...(last ? {} : { next: worker(page + 1).environment }),
      }
    },
  }
  return { asked, client: { devices } as unknown as DevicesClient }
}

describe("devicesGateway list", () => {
  it("reads every page, each after the last worker of the one before", async () => {
    const { asked, client: connected } = client(3)
    const read = await devicesGateway({ connected: async () => connected }).list()
    expect(read).toEqual({
      ok: true,
      value: {
        listening: "0.0.0.0:47650",
        workers: [worker(1), worker(2), worker(3)],
        more: false,
        sshHosts: [],
      },
    })
    expect(asked).toEqual([undefined, worker(1).environment, worker(2).environment])
  })

  it("stops after its last page and says more follow", async () => {
    const { asked, client: connected } = client(0, true)
    const read = await devicesGateway({ connected: async () => connected }).list()
    expect(asked).toHaveLength(MAX_PAGES)
    expect(read.ok && read.value.more).toBe(true)
  })

  it("stops when a page does not go forward, as one cut short", async () => {
    const asked: (string | undefined)[] = []
    const devices = {
      list: async (after?: string) => {
        asked.push(after)
        return { workers: [worker(1)], next: worker(1).environment }
      },
    }
    const connected = { devices } as unknown as DevicesClient
    const read = await devicesGateway({ connected: async () => connected }).list()
    expect(asked).toEqual([undefined, worker(1).environment])
    expect(read.ok && read.value.workers).toEqual([worker(1), worker(1)])
    expect(read.ok && read.value.more).toBe(true)
  })
})

describe("devicesGateway SSH hosts and the default", () => {
  it("reads the first page's hosts and default, and passes connect and default through", async () => {
    const asked: unknown[] = []
    const devices = {
      list: async () => ({
        workers: [],
        sshHosts: [{ host: "box" }],
        defaultEnvironment: "box",
      }),
      sshHosts: async () => ({ suggested: [{ host: "orb" }], connecting: [] }),
      connectSsh: async (host: string) => (
        asked.push(["connect", host]),
        { host, state: "installing" }
      ),
      setDefault: async (environment: string | null) => (
        asked.push(["default", environment]),
        environment
      ),
    }
    const gateway = devicesGateway({
      connected: async () => ({ devices }) as unknown as DevicesClient,
    })
    const read = await gateway.list()
    expect(read.ok && read.value.sshHosts).toEqual([{ host: "box" }])
    expect(read.ok && read.value.defaultEnvironment).toBe("box")
    expect(await gateway.sshHosts()).toEqual({
      ok: true,
      value: { suggested: [{ host: "orb" }], connecting: [] },
    })
    expect(await gateway.connectSsh("orb")).toEqual({
      ok: true,
      value: { host: "orb", state: "installing" },
    })
    expect(await gateway.setDefault(null)).toEqual({ ok: true, value: null })
    expect(asked).toEqual([
      ["connect", "orb"],
      ["default", null],
    ])
  })
})
