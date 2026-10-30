import { describe, expect, it } from "vitest"
import {
  type DeviceMachine,
  ago,
  bytes,
  hubCard,
  sortWorkers,
  summary,
  workerCard,
} from "./devices"

const GB = 1024 ** 3
const machine: DeviceMachine = {
  hostname: "build-box",
  os: "linux",
  osVersion: "24.04",
  arch: "x86_64",
  cpus: 16,
  cpuPermille: 612,
  memoryTotalBytes: 64 * GB,
  memoryUsedBytes: 20 * GB,
  diskTotalBytes: 2048 * GB,
  diskUsedBytes: 512 * GB,
}
const environment = `peer:${"ab".repeat(32)}`

describe("devices model", () => {
  it("reads the hub's usage in people's units", () => {
    const hub = hubCard({
      ...machine,
      hostname: "Studio",
      os: "macos",
      osVersion: "26.1",
      arch: "aarch64",
    })
    expect(hub.name).toBe("Studio")
    expect(hub.system).toBe("macOS 26.1 · Apple silicon")
    expect(hub.cpu).toEqual({ fraction: 0.612, value: "61%", of: "" })
    expect(hub.memory.value).toBe("20 GB")
    expect(hub.memory.of).toBe("of 64 GB")
    expect(hub.disk.of).toBe("of 2 TB")
  })

  it("shows a serving worker's usage and an offline one's last sighting only", () => {
    const online = workerCard({
      environment,
      state: "online",
      agents: 3,
      machine,
      lastSeenMs: 9,
    })
    expect(online).toMatchObject({
      name: "build-box",
      art: "server",
      agents: 3,
      status: "online",
    })
    expect(online.cpu?.value).toBe("61%")
    const offline = workerCard({
      environment,
      state: "offline",
      agents: 2,
      machine,
      lastSeenMs: 9,
    })
    expect(offline.agents).toBe(0)
    expect(offline.cpu).toBeUndefined()
    expect(offline.lastSeenMs).toBe(9)
  })

  it("names a worker that never reported by its key", () => {
    const card = workerCard({ environment, state: "online", agents: 0 })
    expect(card.name).toBe("Worker ababab")
    expect(card.system).toBe("Has not reported yet")
  })

  it("puts online workers first", () => {
    const cards = [
      workerCard({
        environment,
        state: "offline",
        agents: 0,
        machine: { ...machine, hostname: "a" },
      }),
      workerCard({
        environment,
        state: "online",
        agents: 0,
        machine: { ...machine, hostname: "z" },
      }),
    ]
    expect(sortWorkers(cards).map((card) => card.name)).toEqual(["z", "a"])
  })

  it("says sizes, times and the summary plainly", () => {
    expect(bytes(512 * 1024 ** 2)).toBe("512 MB")
    expect(bytes(1.5 * 1024 * GB)).toBe("1.5 TB")
    expect(ago(0, 30_000)).toBe("just now")
    expect(ago(0, 5 * 60_000)).toBe("5 min ago")
    expect(ago(0, 2 * 3_600_000)).toBe("2 h ago")
    expect(ago(0, 72 * 3_600_000)).toBe("3 days ago")
    expect(summary("Studio", [])).toBe(
      "Studio is your hub. Add a computer to run agents on it too.",
    )
    const one = workerCard({ environment, state: "online", agents: 4, machine })
    expect(summary("Studio", [one])).toBe(
      "Studio is your hub. 1 of 1 computer online, 4 agents on them.",
    )
    // Past the list's bound, the rest still count toward the total.
    expect(summary("Studio", [one], 70)).toBe(
      "Studio is your hub. 1 of 71 computers online, 4 agents on them.",
    )
  })
})
