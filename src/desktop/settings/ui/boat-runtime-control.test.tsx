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
    const field = host.querySelector("select")
    if (!field) throw new Error("missing lifetime control")
    field.value = value
    field.dispatchEvent(new Event("change", { bubbles: true }))
  })
}
async function save() {
  await act(async () => {
    const form = host.querySelector("form")
    if (!form) throw new Error("missing runtime settings form")
    form.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }))
  })
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
  await save()
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
  expect(host.querySelector("select")?.value).toBe("28800")
  expect(host.querySelector('button[type="submit"]')?.hasAttribute("disabled")).toBe(true)
  expect(fresh.setRuntimeSettings).not.toHaveBeenCalled()
})
it("preserves a custom saved duration after discarding a preset draft", async () => {
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
  expect(host.querySelector("select")?.value).toBe("12345")
  await select("14400")
  await select("12345")
  await save()
  expect(setRuntimeSettings).not.toHaveBeenCalled()
  expect(host.querySelector("select")?.value).toBe("12345")
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
  expect(host.querySelector("select")?.value).toBe("never")
  await select("14400")
  await save()
  expect(setRuntimeSettings.mock.calls).toEqual([[{ mode: "after", seconds: 14_400 }]])
  expect(host.querySelector("select")?.value).toBe("14400")
  expect(host.querySelector('button[type="submit"]')?.hasAttribute("disabled")).toBe(true)
  expect(host.textContent).toContain("Active VM deadlines stay unchanged")
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
  expect(host.querySelector("select")?.value).toBe("default")
  await select("never")
  await save()
  expect(host.querySelector("select")?.value).toBe("never")
  await select("default")
  await save()
  expect(writes).toEqual([
    { mode: "never", channelId: "ch_bbbbbbbbbb" },
    { mode: "default", channelId: "ch_bbbbbbbbbb" },
  ])
  expect(host.querySelector("select")?.value).toBe("default")
  expect(host.textContent).toContain("Use Nessa default (4 hours)")
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
  expect(host.querySelector("select")?.value).toBe("28800")
  await act(async () => finish({ ...unlimited, channelId: "ch_bbbbbbbbbb" }))
  expect(host.querySelector("select")?.value).toBe("28800")
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
  await save()
  expect(setRuntimeSettings).toHaveBeenCalledTimes(1)
  expect(host.querySelector('[role="alert"]')?.textContent).toContain(
    "could not be confirmed",
  )
  await act(async () =>
    [...host.querySelectorAll("button")]
      .find((button) => button.textContent === "Reload VM settings")
      ?.click(),
  )
  expect(setRuntimeSettings).toHaveBeenCalledTimes(1)
  expect(runtimeSettings).toHaveBeenCalledTimes(2)
  expect(host.querySelector("select")?.value).toBe("14400")
})
