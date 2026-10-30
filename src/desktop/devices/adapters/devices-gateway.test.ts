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
})
