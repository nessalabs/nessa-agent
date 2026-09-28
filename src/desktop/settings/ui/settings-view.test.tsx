// @vitest-environment jsdom
/**
 * Settings as a surface: modal over the window, handing focus back to what
 * opened it; its sidebar folded for room and back with it, the person's
 * choice winning; and no control that does nothing looking as if it works.
 */
import { act, useState } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { settingsCategories, setting, settingsEntries } from "../model/settings-catalogue"
import { settingsTabPages } from "./settings-tabs"
import { SettingsHost } from "./settings-view"
import { testStore } from "../../workspace/testing"

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  Object.assign(globalThis, {
    ResizeObserver: class {
      observe() {}
      disconnect() {}
    },
  })
  Element.prototype.scrollTo ??= () => {}
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener() {},
    removeEventListener() {},
  })) as unknown as typeof window.matchMedia
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})

let setOpen: (open: boolean) => void = () => {}

function Harness() {
  const [open, set] = useState(false)
  setOpen = set
  return (
    <>
      <button type="button" data-opener>
        Open Settings
      </button>
      <SettingsHost
        hostKind="browser"
        browserSurface
        open={open}
        onClose={() => set(false)}
      />
    </>
  )
}

async function mount(width = 1440) {
  Object.defineProperty(window, "innerWidth", { value: width, configurable: true })
  await act(async () =>
    root.render(
      <Provider store={testStore()}>
        <Harness />
      </Provider>,
    ),
  )
}

const settings = () => document.querySelector<HTMLElement>(".settings")
const resize = (width: number) =>
  act(async () => {
    Object.defineProperty(window, "innerWidth", { value: width, configurable: true })
    window.dispatchEvent(new Event("resize"))
  })

describe("Settings over the window", () => {
  it("hands focus back to what opened it when it closes", async () => {
    await mount()
    const opener = host.querySelector<HTMLElement>("[data-opener]")
    opener?.focus()
    await act(async () => setOpen(true))
    expect(document.activeElement).toBe(settings())
    await act(async () => setOpen(false))
    expect(document.activeElement).toBe(opener)
  })

  it("folds its sidebar in a narrow window, and brings it back when the room returns", async () => {
    await mount(550)
    await act(async () => setOpen(true))
    expect(settings()?.dataset.sidebar).toBe("closed")
    await resize(1440)
    expect(settings()?.dataset.sidebar).toBe("open")
  })

  it("lets the person show the folded sidebar at any width, and keeps one they hid hidden", async () => {
    await mount(550)
    await act(async () => setOpen(true))
    const toggle = document.querySelector<HTMLButtonElement>(".settings-titlebar-button")
    await act(async () => toggle?.click())
    expect(settings()?.dataset.sidebar).toBe("open")
    await act(async () => toggle?.click())
    await resize(1440)
    expect(settings()?.dataset.sidebar).toBe("closed")
  })
})

describe("what Settings offers", () => {
  it("disables every control a setting not available yet shows, and says so", async () => {
    const tabs = settingsCategories.flatMap((category) =>
      category.tabs.map((tab) => tab.id),
    )
    await act(async () =>
      root.render(
        <Provider store={testStore()}>
          {tabs.map((tab) => {
            const Page = settingsTabPages[tab]
            return <Page key={tab} />
          })}
        </Provider>,
      ),
    )
    const pending = settingsEntries.filter((entry) => setting(entry.id).pending)
    expect(pending.length).toBeGreaterThan(0)
    for (const entry of pending) {
      const row = host.querySelector(`[data-setting="${entry.id}"]`)
      expect(row, entry.id).not.toBeNull()
      expect(row?.textContent).toContain("Not available yet")
      expect(row?.querySelectorAll("button:not(:disabled)").length, entry.id).toBe(0)
    }
    // And every other control can be used.
    const available = [...host.querySelectorAll("[data-setting]:not([data-pending])")]
    expect(
      available.every((row) => row.querySelectorAll("button:disabled").length === 0),
    ).toBe(true)
  })
})
