// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { beforeEach, afterEach, expect, it, vi } from "vitest"
import type { BoatRuntimeSetParams, BoatRuntimeSettings } from "@nessa/client"
import type { BoatGateway } from "../adapters/boat-gateway"
import { BoatRuntimeControl } from "./boat-runtime-control"
let root: Root
let host: HTMLDivElement
beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})
afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})
const unlimited: BoatRuntimeSettings = { defaultLifetime: { mode: "never" } }
async function select(value: string) {
  await act(async () => {
    const option = host.querySelector<HTMLButtonElement>(`[data-value="${value}"]`)
    if (!option) throw new Error(`missing lifetime choice ${value}`)
    option.click()
  })
}
/** The choice the control shows as made. */
function chosen() {
  return host.querySelector<HTMLElement>("[data-boat-runtime-settings]")?.dataset
    .selection
}
it("a late save cannot replace a newly connected gateway's settings", async () => {
  let finish: ((value: BoatRuntimeSettings) => void) | undefined
  const old = {
    runtimeSettings: async () => unlimited,
    setRuntimeSettings: () =>
      new Promise<BoatRuntimeSettings>((resolve) => {
        finish = resolve
      }),
  }
  await act(async () => root.render(<BoatRuntimeControl gateway={old} />))
  await select("14400")
  expect(finish).toBeDefined()
  const fresh = {
    runtimeSettings: async (): Promise<BoatRuntimeSettings> => ({
      defaultLifetime: { mode: "after", seconds: 28_800 },
    }),
    setRuntimeSettings: vi.fn(async (): Promise<BoatRuntimeSettings> => {
      throw new Error("unexpected save")
    }),
  }
  await act(async () => root.render(<BoatRuntimeControl gateway={fresh} />))
  await act(async () => finish?.({ defaultLifetime: { mode: "after", seconds: 14_400 } }))
  expect(chosen()).toBe("28800")
  expect(fresh.setRuntimeSettings).not.toHaveBeenCalled()
})
it("choosing the saved custom duration again saves nothing", async () => {
  const setRuntimeSettings = vi.fn(async (): Promise<BoatRuntimeSettings> => {
    throw new Error("unexpected save")
  })
  const gateway = {
    runtimeSettings: async (): Promise<BoatRuntimeSettings> => ({
      defaultLifetime: { mode: "after", seconds: 12_345 },
    }),
    setRuntimeSettings,
  }
  await act(async () => root.render(<BoatRuntimeControl gateway={gateway} />))
  expect(chosen()).toBe("12345")
  expect(host.textContent).toContain("12345s")
  await select("12345")
  expect(setRuntimeSettings).not.toHaveBeenCalled()
  expect(chosen()).toBe("12345")
})
it("saves an ordinary global default without receiving token capabilities", async () => {
  let stored = unlimited
  const setRuntimeSettings = vi.fn(
    async (params: BoatRuntimeSetParams): Promise<BoatRuntimeSettings> => {
      stored = {
        defaultLifetime:
          params.mode === "after"
            ? { mode: "after", seconds: params.seconds }
            : { mode: "never" },
      }
      return stored
    },
  )
  const gateway = { runtimeSettings: async () => stored, setRuntimeSettings }
  await act(async () => root.render(<BoatRuntimeControl gateway={gateway} />))
  expect(chosen()).toBe("never")
  await select("14400")
  expect(setRuntimeSettings.mock.calls).toEqual([[{ mode: "after", seconds: 14_400 }]])
  expect(chosen()).toBe("14400")
  expect(host.textContent).not.toContain("keep billing")
  await select("never")
  expect(host.textContent).toContain("VMs keep billing until you stop them in Boat.")
})
it("keeps explicit unlimited separate from inheritance and resets one channel", async () => {
  let stored: BoatRuntimeSettings = {
    channelId: "ch_bbbbbbbbbb",
    defaultLifetime: { mode: "after", seconds: 3600 },
  }
  const writes: BoatRuntimeSetParams[] = []
  const gateway = {
    runtimeSettings: async () => stored,
    setRuntimeSettings: async (
      params: BoatRuntimeSetParams,
    ): Promise<BoatRuntimeSettings> => {
      writes.push(params)
      stored =
        params.mode === "default"
          ? {
              channelId: params.channelId,
              defaultLifetime: { mode: "after", seconds: 14_400 },
            }
          : {
              channelId: params.channelId,
              defaultLifetime: { mode: "after", seconds: 3600 },
              channelOverride: { mode: "never" },
            }
      return stored
    },
  }
  await act(async () =>
    root.render(<BoatRuntimeControl gateway={gateway} channelId="ch_bbbbbbbbbb" />),
  )
  expect(chosen()).toBe("default")
  await select("never")
  expect(chosen()).toBe("never")
  await select("default")
  expect(writes).toEqual([
    { mode: "never", channelId: "ch_bbbbbbbbbb" },
    { mode: "default", channelId: "ch_bbbbbbbbbb" },
  ])
  expect(chosen()).toBe("default")
  expect(host.textContent).toContain("Default: 4 hours")
})
it("ignores a late read from another channel", async () => {
  let finish!: (value: BoatRuntimeSettings) => void
  const gateway: Pick<BoatGateway, "runtimeSettings" | "setRuntimeSettings"> = {
    runtimeSettings: async (params) =>
      params?.channelId === "ch_bbbbbbbbbb"
        ? new Promise((resolve) => {
            finish = resolve
          })
        : {
            channelId: "ch_cccccccccc",
            defaultLifetime: { mode: "after", seconds: 3600 },
            channelOverride: { mode: "after", seconds: 28_800 },
          },
    setRuntimeSettings: async () => unlimited,
  }
  await act(async () =>
    root.render(<BoatRuntimeControl gateway={gateway} channelId="ch_bbbbbbbbbb" />),
  )
  await act(async () =>
    root.render(<BoatRuntimeControl gateway={gateway} channelId="ch_cccccccccc" />),
  )
  expect(chosen()).toBe("28800")
  await act(async () => finish({ ...unlimited, channelId: "ch_bbbbbbbbbb" }))
  expect(chosen()).toBe("28800")
})
it("requires an explicit reload after an uncertain save and does not replay it", async () => {
  let stored = unlimited
  const runtimeSettings = vi.fn(async () => stored)
  const setRuntimeSettings = vi.fn(async (): Promise<BoatRuntimeSettings> => {
    stored = { defaultLifetime: { mode: "after", seconds: 14_400 } }
    throw new Error("lost reply")
  })
  const gateway = { runtimeSettings, setRuntimeSettings }
  await act(async () => root.render(<BoatRuntimeControl gateway={gateway} />))
  await select("14400")
  expect(setRuntimeSettings).toHaveBeenCalledTimes(1)
  expect(host.querySelector('[role="alert"]')?.textContent).toContain("Couldn’t confirm")
  await act(async () =>
    [...host.querySelectorAll("button")]
      .find((button) => button.textContent === "Reload")
      ?.click(),
  )
  expect(setRuntimeSettings).toHaveBeenCalledTimes(1)
  expect(runtimeSettings).toHaveBeenCalledTimes(2)
  expect(chosen()).toBe("14400")
})
