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
/** Opens the lifetime menu by keyboard, as a person can, and picks `value`. */
async function select(value: string) {
  const button = host.querySelector<HTMLButtonElement>("[data-choice-trigger]")
  if (!button) throw new Error("missing lifetime menu")
  if (button.disabled) return
  await act(async () => {
    button.focus()
    button.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
    )
  })
  const option = document.querySelector<HTMLElement>(
    `[role="menuitemradio"][data-value="${value}"]`,
  )
  if (!option) throw new Error(`missing lifetime choice ${value}`)
  await act(async () => option.click())
}
/** The amount's field, shown while auto-stop is a length. */
function field() {
  return host.querySelector<HTMLInputElement>("[data-boat-runtime-amount]")
}
/** Types `text` into the amount's field, leaving it focused. */
async function type(text: string) {
  const input = field()
  if (!input) throw new Error("missing amount field")
  await act(async () => {
    input.focus()
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(
      input,
      text,
    )
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
  return input
}
/** Types `text` into the amount's field and enters it with ↩. */
async function enter(text: string) {
  const input = await type(text)
  await act(async () => {
    input.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
    )
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
  await select("hours")
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
  await act(async () => finish?.({ defaultLifetime: { mode: "after", seconds: 3600 } }))
  expect(chosen()).toBe("28800")
  expect(fresh.setRuntimeSettings).not.toHaveBeenCalled()
})
it("entering the saved custom duration again saves nothing", async () => {
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
  expect(field()?.value).toBe("12345")
  expect(host.querySelector("[data-choice-trigger]")?.textContent).toContain("seconds")
  await enter("12345")
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
  expect(field()).toBeNull()
  // A unit picked from Never starts at one of it.
  await select("hours")
  expect(field()?.value).toBe("1")
  await enter("4")
  // The unit keeps the amount: 4 hours, then 4 days.
  await select("days")
  expect(setRuntimeSettings.mock.calls).toEqual([
    [{ mode: "after", seconds: 3600 }],
    [{ mode: "after", seconds: 14_400 }],
    [{ mode: "after", seconds: 345_600 }],
  ])
  expect(chosen()).toBe("345600")
  await select("minutes")
  await enter("90")
  expect(chosen()).toBe("5400")
  // 90 minutes reads back as 90 minutes, not 1.5 hours.
  expect(field()?.value).toBe("90")
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
    stored = { defaultLifetime: { mode: "after", seconds: 3600 } }
    throw new Error("lost reply")
  })
  const gateway = { runtimeSettings, setRuntimeSettings }
  await act(async () => root.render(<BoatRuntimeControl gateway={gateway} />))
  await select("hours")
  expect(setRuntimeSettings).toHaveBeenCalledTimes(1)
  expect(host.querySelector('[role="alert"]')?.textContent).toContain("Couldn’t confirm")
  await act(async () =>
    [...host.querySelectorAll("button")]
      .find((button) => button.textContent === "Reload")
      ?.click(),
  )
  expect(setRuntimeSettings).toHaveBeenCalledTimes(1)
  expect(runtimeSettings).toHaveBeenCalledTimes(2)
  expect(chosen()).toBe("3600")
})
it("refuses an amount that is not a whole number or too long, saving nothing", async () => {
  let stored: BoatRuntimeSettings = { defaultLifetime: { mode: "after", seconds: 7200 } }
  const setRuntimeSettings = vi.fn(
    async (params: BoatRuntimeSetParams): Promise<BoatRuntimeSettings> => {
      stored = { defaultLifetime: { mode: "after", seconds: params.seconds } }
      return stored
    },
  )
  const gateway = { runtimeSettings: async () => stored, setRuntimeSettings }
  await act(async () => root.render(<BoatRuntimeControl gateway={gateway} />))
  expect(field()?.value).toBe("2")
  for (const [text, says] of [
    ["0", "whole number of hours"],
    ["1.5", "whole number of hours"],
    ["two", "whole number of hours"],
    ["", "whole number of hours"],
    ["1193047", "Up to 1,193,046 hours"],
  ] as const) {
    await enter(text)
    expect(host.querySelector('[role="alert"]')?.textContent).toContain(says)
    expect(field()?.getAttribute("aria-invalid")).toBe("true")
  }
  // A unit that makes the typed amount too long is refused too.
  await enter("50000")
  await select("days")
  expect(host.querySelector('[role="alert"]')?.textContent).toContain("Up to 49,710 days")
  expect(setRuntimeSettings.mock.calls).toEqual([
    [{ mode: "after", seconds: 180_000_000 }],
  ])
  await enter("49710")
  expect(setRuntimeSettings).toHaveBeenLastCalledWith({
    mode: "after",
    seconds: 4_294_944_000,
  })
  expect(host.querySelector('[role="alert"]')).toBeNull()
})
it("saves a unit picked while the amount is still saving, after it", async () => {
  let stored: BoatRuntimeSettings = { defaultLifetime: { mode: "after", seconds: 3600 } }
  const answers: (() => void)[] = []
  const setRuntimeSettings = vi.fn(
    (params: BoatRuntimeSetParams) =>
      new Promise<BoatRuntimeSettings>((resolve) => {
        answers.push(() => {
          stored = { defaultLifetime: { mode: "after", seconds: params.seconds } }
          resolve(stored)
        })
      }),
  )
  const gateway = { runtimeSettings: async () => stored, setRuntimeSettings }
  await act(async () => root.render(<BoatRuntimeControl gateway={gateway} />))
  // Typed, not entered: opening the unit's menu takes focus from the field,
  // which saves the amount while the menu is open.
  await type("3")
  await select("days")
  expect(setRuntimeSettings).toHaveBeenCalledTimes(1)
  await act(async () => answers.shift()?.())
  await act(async () => answers.shift()?.())
  expect(setRuntimeSettings.mock.calls).toEqual([
    [{ mode: "after", seconds: 10_800 }],
    [{ mode: "after", seconds: 259_200 }],
  ])
  expect(chosen()).toBe("259200")
})
it("keeps an entered amount in its unit, so a unit picked after the save applies to it", async () => {
  let stored: BoatRuntimeSettings = { defaultLifetime: { mode: "after", seconds: 3600 } }
  const setRuntimeSettings = vi.fn(
    async (params: BoatRuntimeSetParams): Promise<BoatRuntimeSettings> => {
      stored = { defaultLifetime: { mode: "after", seconds: params.seconds } }
      return stored
    },
  )
  const gateway = { runtimeSettings: async () => stored, setRuntimeSettings }
  await act(async () => root.render(<BoatRuntimeControl gateway={gateway} />))
  // 24 hours is a whole day, but stays 24 hours as entered.
  await enter("24")
  expect(field()?.value).toBe("24")
  expect(host.querySelector("[data-choice-trigger]")?.textContent).toContain("hours")
  await select("days")
  expect(setRuntimeSettings.mock.calls).toEqual([
    [{ mode: "after", seconds: 86_400 }],
    [{ mode: "after", seconds: 2_073_600 }],
  ])
})
