// @vitest-environment jsdom
import { act, StrictMode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { DevicesListResult } from "@nessa/client"
import { ClockProvider } from "../../../clock"
import type { DevicesGateway, WorkerPairing } from "../adapters/devices-gateway"
import { DevicesProvider, DevicesView } from "./devices-view"
import { commands, hubAddress, listeningPort, onlyThisMachine } from "./add-computer"

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

beforeEach(() => {
  vi.useFakeTimers()
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia
  Element.prototype.animate ??= function () {
    return { cancel() {}, finished: Promise.resolve() } as unknown as Animation
  }
})

afterEach(() => {
  vi.useRealTimers()
  document.body.replaceChildren()
})

const GB = 1024 ** 3
const machine = (hostname: string, os = "linux") => ({
  hostname,
  os,
  osVersion: "24.04",
  arch: "x86_64",
  cpus: 16,
  cpuPermille: 610,
  memoryTotalBytes: 64 * GB,
  memoryUsedBytes: 20 * GB,
  diskTotalBytes: 1024 * GB,
  diskUsedBytes: 256 * GB,
})
const KEY = "ab".repeat(32)
const listed: DevicesListResult = {
  hub: { ...machine("Studio", "macos"), osVersion: "26.1", arch: "aarch64" },
  workers: [
    {
      environment: `peer:${"cd".repeat(32)}`,
      state: "offline",
      agents: 0,
      machine: machine("gaming-pc", "windows"),
      lastSeenMs: 1_000_000 - 2 * 3_600_000,
    },
    {
      environment: `peer:${KEY}`,
      state: "online",
      agents: 4,
      machine: machine("build-box"),
      lastSeenMs: 999_000,
    },
  ],
  omitted: 0,
}

function pairing(phase: WorkerPairing["phase"], claimed = false): WorkerPairing {
  return {
    invitationId: "11".repeat(16),
    phase,
    expiresAtMs: 1_000_000 + 600_000,
    ...(claimed ? { deviceKey: KEY, fingerprint: "9f3ac27e41bd0e88" } : {}),
  }
}

function fakeGateway(overrides: Partial<DevicesGateway> = {}) {
  const calls: string[] = []
  const gateway: DevicesGateway = {
    pollMs: 5_000,
    list: async () => (calls.push("list"), { ok: true, value: listed }),
    cloud: async () => ({ ok: true, value: "connected" }),
    linked: async () => ({
      ok: true,
      value: [{ credentialId: "c1", issuedAt: 1_700_000_000, fullAccess: true }],
    }),
    pair: async () => (
      calls.push("pair"),
      { ok: true, value: { code: "ABCD-2345", pairing: pairing("available") } }
    ),
    status: async () => (
      calls.push("status"),
      { ok: true, value: pairing("claimed", true) }
    ),
    approve: async (_, key) => (
      calls.push(`approve ${key.slice(0, 4)}`),
      { ok: true, value: pairing("active", true) }
    ),
    cancel: async () => (calls.push("cancel"), { ok: true, value: pairing("terminal") }),
    ...overrides,
  }
  return { gateway, calls }
}

async function mount(
  gateway: DevicesGateway,
  strict = false,
): Promise<{ host: HTMLElement; root: Root }> {
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  const view = (
    <ClockProvider now={() => 1_000_000}>
      <DevicesProvider gateway={gateway}>
        <DevicesView />
      </DevicesProvider>
    </ClockProvider>
  )
  await act(async () => root.render(strict ? <StrictMode>{view}</StrictMode> : view))
  await act(async () => {})
  return { host, root }
}

const text = (host: HTMLElement, selector: string) =>
  host.querySelector(selector)?.textContent ?? null

it("lists the hub, its computers online first, the cloud and the linked phone", async () => {
  const { host, root } = await mount(fakeGateway().gateway)
  expect(text(host, "[data-devices-summary]")).toBe(
    "Studio is your hub. 1 of 2 computers online, 4 agents on them.",
  )
  expect(text(host, "#devices-hub-name")).toBe("Studio")
  expect(text(host, '[data-devices-stat="CPU"] .devices-stat-value')).toBe("61%")
  const tiles = [...host.querySelectorAll("[data-devices-worker]")].map((tile) =>
    tile.getAttribute("aria-label"),
  )
  expect(tiles).toEqual(["build-box, Online", "gaming-pc, Offline"])
  expect(
    text(host, `[data-devices-worker="${"cd".repeat(32)}"] .devices-tile-foot`),
  ).toBe("Last seen 2 h ago")
  expect(
    host
      .querySelector(
        `[data-devices-worker="${"cd".repeat(32)}"] [data-slot="device-figure"]`,
      )
      ?.getAttribute("data-dimmed"),
  ).toBe("true")
  expect(
    host.querySelector("[data-devices-cloud]")?.getAttribute("data-devices-cloud"),
  ).toBe("connected")
  expect(host.querySelector('[data-devices-linked="c1"]')).not.toBeNull()
  await act(async () => root.unmount())
  // Nothing keeps reading once the page is gone.
  expect(vi.getTimerCount()).toBe(0)
})

it("reads again every poll while open", async () => {
  const { gateway, calls } = fakeGateway()
  const { root } = await mount(gateway)
  expect(calls.filter((call) => call === "list")).toHaveLength(1)
  await act(async () => vi.advanceTimersByTime(5_000))
  expect(calls.filter((call) => call === "list")).toHaveLength(2)
  await act(async () => root.unmount())
})

it("opens a computer's page with its usage and no list of what runs there", async () => {
  const { host, root } = await mount(fakeGateway().gateway)
  await act(async () =>
    (host.querySelector(`[data-devices-worker="${KEY}"]`) as HTMLElement).click(),
  )
  expect(host.querySelector('[data-devices-page="worker"] h1')?.textContent).toBe(
    "build-box",
  )
  expect(text(host, '[data-devices-stat="Agents"] .devices-stat-value')).toBe("4")
  expect(host.textContent).not.toContain("Running here")
  await act(async () =>
    (host.querySelector('[data-devices-action="back"]') as HTMLElement).click(),
  )
  expect(host.querySelector('[data-devices-page="list"]')).not.toBeNull()
  await act(async () => root.unmount())
})

it("adds a computer: a code, its commands, then approving the key that used it", async () => {
  const { gateway, calls } = fakeGateway()
  const { host, root } = await mount(gateway)
  await act(async () =>
    (host.querySelector('[data-devices-action="add"]') as HTMLElement).click(),
  )
  await act(async () => {})
  const shown = [...host.querySelectorAll("[data-devices-command] code")].map(
    (code) => code.textContent,
  )
  expect(shown).toEqual([
    "nessa env enroll studio:47650 ABCD-2345",
    "nessa env dial studio:47650",
  ])
  expect(host.querySelector('[data-devices-action="approve"]')).toBeNull()
  await act(async () => vi.advanceTimersByTime(2_000))
  await act(async () => {})
  await act(async () =>
    (host.querySelector('[data-devices-action="approve"]') as HTMLElement).click(),
  )
  await act(async () => {})
  expect(calls).toContain("approve abab")
  expect(host.querySelector("[data-devices-added]")).not.toBeNull()
  await act(async () => root.unmount())
  // Approved: nothing is left to cancel.
  expect(calls).not.toContain("cancel")
})

it("cancels the code when the page closes before approving", async () => {
  const { gateway, calls } = fakeGateway()
  const { host, root } = await mount(gateway)
  await act(async () =>
    (host.querySelector('[data-devices-action="add"]') as HTMLElement).click(),
  )
  await act(async () => {})
  await act(async () => root.unmount())
  expect(calls).toContain("cancel")
  expect(host.textContent).toBe("")
})

it("asks for one code at a time, as the hub makes them", async () => {
  // The hub refuses a code while it makes another (`pairing_busy`); a page
  // mounted twice, as React's strict mode does, must not ask twice at once.
  let making = false
  const { gateway, calls } = fakeGateway({
    pair: async () => {
      if (making) return (calls.push("busy"), { ok: false })
      making = true
      await Promise.resolve()
      making = false
      calls.push("pair")
      return { ok: true, value: { code: "ABCD-2345", pairing: pairing("available") } }
    },
  })
  const { host } = await mount(gateway, true)
  await act(async () =>
    (host.querySelector('[data-devices-action="add"]') as HTMLElement).click(),
  )
  for (let i = 0; i < 4; i += 1) await act(async () => {})
  expect(calls).not.toContain("busy")
  expect(host.querySelectorAll("[data-devices-command]")).toHaveLength(2)
})

it("leaving while approving keeps the approval, and cancels a refused one", async () => {
  for (const accepted of [true, false]) {
    let answer: (value: Awaited<ReturnType<DevicesGateway["approve"]>>) => void = () => {}
    const { gateway, calls } = fakeGateway({
      approve: (_, key) => {
        calls.push(`approve ${key.slice(0, 4)}`)
        return new Promise((resolve) => (answer = resolve))
      },
    })
    const { host, root } = await mount(gateway)
    await act(async () =>
      (host.querySelector('[data-devices-action="add"]') as HTMLElement).click(),
    )
    await act(async () => {})
    await act(async () => vi.advanceTimersByTime(2_000))
    await act(async () => {})
    await act(async () =>
      (host.querySelector('[data-devices-action="approve"]') as HTMLElement).click(),
    )
    await act(async () => root.unmount())
    expect(calls).not.toContain("cancel")
    await act(async () =>
      answer(accepted ? { ok: true, value: pairing("active", true) } : { ok: false }),
    )
    // Approved, there is nothing to cancel; refused, nobody else will.
    expect(calls.includes("cancel")).toBe(!accepted)
    document.body.replaceChildren()
  }
})

it("dials the port the hub listens on, and says when only it can reach that", () => {
  expect(listeningPort("0.0.0.0:47700")).toBe(47700)
  expect(listeningPort("[::]:47701")).toBe(47701)
  expect(listeningPort(undefined)).toBe(47650)
  expect(hubAddress("Studio", "tailscale", 47700)).toBe("studio:47700")
  expect(onlyThisMachine("127.0.0.1:47650")).toBe(true)
  expect(onlyThisMachine("[::1]:47650")).toBe(true)
  expect(onlyThisMachine("0.0.0.0:47650")).toBe(false)
  expect(onlyThisMachine(undefined)).toBe(false)
})

it("writes the address each route dials", () => {
  expect(hubAddress("Saurav’s Mac Studio", "tailscale")).toBe("sauravs-mac-studio:47650")
  expect(hubAddress("studio.local", "network")).toBe("studio.local:47650")
  expect(commands("h:1", "C")).toEqual(["nessa env enroll h:1 C", "nessa env dial h:1"])
})
