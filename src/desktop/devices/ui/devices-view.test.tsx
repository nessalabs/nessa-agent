// @vitest-environment jsdom
import type { BoatGateway } from "../../settings/adapters/boat-gateway"
import { BoatProvider } from "../../settings/ui/boat-row"
import { act, StrictMode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { ClockProvider } from "../../../clock"
import type { DevicesGateway, DevicesRead } from "../adapters/devices-gateway"
import { DevicesProvider, DevicesView } from "./devices-view"

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
const listed: DevicesRead = {
  hub: {
    hostname: "Studio",
    os: "macos",
    osVersion: "26.1",
    arch: "aarch64",
    cpus: 16,
    cpuPermille: 610,
    memoryTotalBytes: 64 * GB,
    memoryUsedBytes: 20 * GB,
    diskTotalBytes: 1024 * GB,
    diskUsedBytes: 256 * GB,
  },
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
    ...overrides,
  }
  return { gateway, calls }
}

async function mount(
  gateway: DevicesGateway,
  strict = false,
  boat?: BoatGateway,
): Promise<{ host: HTMLElement; root: Root }> {
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  const view = (
    <ClockProvider now={() => 1_000_000} elapsed={() => 1_000_000}>
      <BoatProvider gateway={boat}>
        <DevicesProvider gateway={gateway}>
          <DevicesView />
        </DevicesProvider>
      </BoatProvider>
    </ClockProvider>
  )
  await act(async () => root.render(strict ? <StrictMode>{view}</StrictMode> : view))
  await act(async () => {})
  return { host, root }
}

const text = (host: HTMLElement, selector: string) =>
  host.querySelector(selector)?.textContent ?? null

it("lists the hub, the cloud and the linked phone, and no other computer", async () => {
  const { host, root } = await mount(fakeGateway().gateway)
  // The title stands alone: no line of instructions under it.
  expect(host.querySelector(".devices-heading p")).toBeNull()
  expect(text(host, "#devices-hub-name")).toBe("Studio")
  expect(text(host, '[data-devices-stat="CPU"] .devices-stat-value')).toBe("61%")
  // Every gateway runs only its own agents (ADR 1417): no computer works
  // for this one, so none is listed.
  expect(host.querySelector("[data-devices-worker]")).toBeNull()
  expect(
    host.querySelector("[data-devices-cloud]")?.getAttribute("data-devices-cloud"),
  ).toBe("connected")
  expect(host.querySelector('[data-devices-linked="c1"]')).not.toBeNull()
  await act(async () => root.unmount())
  // Nothing keeps reading once the page is gone.
  expect(vi.getTimerCount()).toBe(0)
})

it("shows every device in one list, each saying what it is, and one Add device", async () => {
  const { host, root } = await mount(fakeGateway().gateway)
  const list = host.querySelector("[data-devices-list]")
  expect(
    [...(list?.children ?? [])]
      .map(
        // A tile with a star is held beside it.
        (child) => child.querySelector(":scope > .devices-tile") ?? child,
      )
      .map((tile) =>
        tile.hasAttribute("data-devices-cloud")
          ? "cloud"
          : tile.hasAttribute("data-devices-linked")
            ? "phone"
            : tile.getAttribute("data-devices-action"),
      ),
  ).toEqual(["cloud", "phone"])
  expect(text(host, "[data-devices-cloud] .devices-tile-body .devices-muted")).toBe(
    "Cloud VMs",
  )
  expect(host.querySelectorAll("h2:not(#devices-hub-name)")).toHaveLength(0)
  expect(text(host, '[data-devices-action="add"]')).toBe("Add device")
  expect(host.querySelectorAll('[data-devices-action^="add"]')).toHaveLength(1)
  await act(async () => root.unmount())
})

it("manages Boat from its own page, with no trip to Settings", async () => {
  const calls: string[] = []
  const boat: BoatGateway = {
    read: async () => ({ state: "connected", environment: "nessa" }),
    save: async () => ({ state: "connected", environment: "nessa" }),
    disconnect: async () => {
      calls.push("disconnect")
      return { state: "disconnected" }
    },
  }
  // Not connected is where the list once said "Connect in Settings ›".
  const { gateway, calls: reads } = fakeGateway({
    cloud: async () => ({ ok: true, value: "disconnected" }),
  })
  const { host, root } = await mount(gateway, false, boat)
  // One word for where Boat stands, not "Offline" and "Not connected" both.
  expect(text(host, "[data-devices-cloud] .devices-status")).toBe("Not connected")
  expect(host.querySelector("[data-devices-cloud] .devices-tile-foot")).toBeNull()
  expect(host.querySelector("[data-devices-cloud]")?.textContent).not.toContain("Offline")
  expect(host.textContent).not.toContain("Settings ›")
  // Boat is drawn as a boat; one not connected sits still.
  const tileBoat = host.querySelector("[data-devices-cloud] [data-slot=device-figure]")
  expect(tileBoat?.getAttribute("data-kind")).toBe("boat")
  expect(tileBoat?.hasAttribute("data-afloat")).toBe(false)
  await act(async () =>
    (host.querySelector("[data-devices-cloud]") as HTMLElement).click(),
  )
  await act(async () => {})
  expect(host.querySelector('[data-devices-page="cloud"] h1')?.textContent).toBe("Boat")
  expect(text(host, "[data-devices-cloud-state]")).toContain("Connected")
  expect(host.querySelector('input[type="password"]')).not.toBeNull()
  const pageBoat = () =>
    host.querySelector('[data-devices-page="cloud"] [data-slot=device-figure]')
  expect(pageBoat()?.getAttribute("data-afloat")).toBe("true")
  const before = reads.filter((call) => call === "list").length
  await act(async () =>
    (host.querySelector('[data-devices-action="disconnect"]') as HTMLElement).click(),
  )
  await act(async () => {})
  expect(calls).toEqual(["disconnect"])
  expect(text(host, "[data-devices-cloud-state]")).toContain("Not connected")
  expect(pageBoat()?.hasAttribute("data-afloat")).toBe(false)
  // The list reads again, so going back shows the new state.
  expect(reads.filter((call) => call === "list").length).toBeGreaterThan(before)
  await act(async () => root.unmount())
})

it("offers Unlock, not a token field, while the vault is locked, then reads Boat again", async () => {
  let locked = true
  let cloud: "locked" | "connected" = "locked"
  const boat: BoatGateway = {
    read: async () =>
      locked ? { state: "locked" } : { state: "connected", environment: "nessa" },
    save: async () => ({ state: "connected", environment: "nessa" }),
    disconnect: async () => ({ state: "disconnected" }),
  }
  const { gateway } = fakeGateway({ cloud: async () => ({ ok: true, value: cloud }) })
  const { host, root } = await mount(gateway, false, boat)
  expect(text(host, "[data-devices-cloud] .devices-status")).toBe("Locked")
  await act(async () =>
    (host.querySelector("[data-devices-cloud]") as HTMLElement).click(),
  )
  await act(async () => {})
  expect(text(host, "[data-devices-cloud-state]")).toContain("Locked")
  expect(host.querySelector('input[type="password"]')).toBeNull()
  expect(host.querySelector('[data-boat-action="unlock"]')).not.toBeNull()
  // The vault opens elsewhere: the list's next read notices, and the page reads again.
  locked = false
  cloud = "connected"
  await act(async () => vi.advanceTimersByTime(5_000))
  await act(async () => {})
  expect(text(host, "[data-devices-cloud-state]")).toContain("Connected")
  expect(host.querySelector('input[type="password"]')).not.toBeNull()
  await act(async () => root.unmount())
})

it("says Unavailable, not Reading, when there is no Boat to read", async () => {
  const { host, root } = await mount(fakeGateway().gateway)
  await act(async () =>
    (host.querySelector("[data-devices-cloud]") as HTMLElement).click(),
  )
  await act(async () => {})
  expect(text(host, "[data-devices-cloud-state]")).toContain("Unavailable")
  await act(async () => root.unmount())
})

it("adds a phone or the cloud from Add device, and no computer", async () => {
  const { host, root } = await mount(fakeGateway().gateway)
  await act(async () =>
    (host.querySelector('[data-devices-action="add"]') as HTMLElement).click(),
  )
  expect(
    [...host.querySelectorAll("[data-devices-add]")].map((each) =>
      each.getAttribute("data-devices-add"),
    ),
  ).toEqual(["phone", "cloud"])
  await act(async () =>
    (host.querySelector('[data-devices-add="phone"]') as HTMLElement).click(),
  )
  expect(host.querySelector('[data-devices-page="add-phone"]')).not.toBeNull()
  expect(host.querySelector('[data-setting="linked-devices"]')).not.toBeNull()
  await act(async () =>
    (host.querySelector('[data-devices-action="back"]') as HTMLElement).click(),
  )
  await act(async () =>
    (host.querySelector('[data-devices-action="add"]') as HTMLElement).click(),
  )
  await act(async () =>
    (host.querySelector('[data-devices-add="cloud"]') as HTMLElement).click(),
  )
  // Every cloud in the registry is offered, each leading to its own page.
  expect(
    [...host.querySelectorAll("[data-devices-add-cloud]")].map((each) =>
      each.getAttribute("data-devices-add-cloud"),
    ),
  ).toEqual(["boat", "agent37"])
  await act(async () =>
    (host.querySelector('[data-devices-add-cloud="agent37"]') as HTMLElement).click(),
  )
  expect(host.querySelector('[data-devices-cloud-page="agent37"]')).not.toBeNull()
  expect(host.querySelector('[data-setting="agent37"]')).not.toBeNull()
  await act(async () =>
    (host.querySelector('[data-devices-action="back"]') as HTMLElement).click(),
  )
  await act(async () =>
    (host.querySelector('[data-devices-action="add"]') as HTMLElement).click(),
  )
  await act(async () =>
    (host.querySelector('[data-devices-add="cloud"]') as HTMLElement).click(),
  )
  await act(async () =>
    (host.querySelector('[data-devices-add-cloud="boat"]') as HTMLElement).click(),
  )
  expect(host.querySelector('[data-devices-page="cloud"]')).not.toBeNull()
  expect(host.querySelector('[data-devices-cloud-page="agent37"]')).toBeNull()
  await act(async () => root.unmount())
})

it("reads again every poll while open", async () => {
  const { gateway, calls } = fakeGateway()
  const { root } = await mount(gateway)
  expect(calls.filter((call) => call === "list")).toHaveLength(1)
  await act(async () => vi.advanceTimersByTime(5_000))
  expect(calls.filter((call) => call === "list")).toHaveLength(2)
  await act(async () => root.unmount())
})
