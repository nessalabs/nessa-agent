import { describe, expect, it } from "vitest"
import { type DeviceMachine, bytes, hubCard, statusWord } from "./devices"

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

  it("names a hub that says no name as this Mac", () => {
    expect(hubCard({ ...machine, hostname: "" }).name).toBe("This Mac")
    expect(hubCard(machine).system).toBe("Linux 24.04")
  })

  it("says sizes and states plainly", () => {
    expect(bytes(512 * 1024 ** 2)).toBe("512 MB")
    expect(bytes(1.5 * 1024 * GB)).toBe("1.5 TB")
    expect(statusWord("ready")).toBe("Ready")
    expect(statusWord("offline")).toBe("Offline")
  })
})
