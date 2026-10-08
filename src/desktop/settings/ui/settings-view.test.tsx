// @vitest-environment jsdom
/**
 * Settings as a surface: modal over the window, handing focus back to what
 * opened it; its sidebar folded for room and back with it, the person's
 * choice winning; and no control that does nothing looking as if it works.
 */
import { act, useState } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { chordEvent } from "../../model/keyboard"
import { settingsCategories, setting, settingsEntries } from "../model/settings-catalogue"
import { settingsTabPages } from "./settings-tabs"
import { SettingsHost } from "./settings-view"
import { testStore } from "../../workspace/testing"

// The platform the window read, as each test says: a Mac unless it says otherwise.
const platform = vi.hoisted(() => ({ mac: true }))
vi.mock("../../adapters/platform", () => ({
  get isMac() {
    return platform.mac
  },
}))

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
  platform.mac = true
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

describe("the keys Settings names", () => {
  for (const [mac, sidebar, sessionList, open] of [
    [true, "⌘B", "⌥⌘S", "⌘,"],
    [false, "Ctrl+B", "Ctrl+Alt+S", "Ctrl+,"],
  ] as const)
    it(`writes them as ${mac ? "a Mac" : "elsewhere"} does, and takes the key it names`, async () => {
      platform.mac = mac
      await mount()
      await act(async () => setOpen(true))
      const toggle = document.querySelector<HTMLButtonElement>(
        ".settings-titlebar-button",
      )
      expect(toggle?.getAttribute("aria-label")).toBe(`Hide Sidebar (${sidebar})`)
      // The key the label names is the key that works.
      await act(async () => {
        window.dispatchEvent(
          new KeyboardEvent("keydown", {
            ...chordEvent({ code: "KeyB", command: true }, mac),
            bubbles: true,
          }),
        )
      })
      expect(settings()?.dataset.sidebar).toBe("closed")
      await act(async () =>
        root.render(
          <Provider store={testStore()}>
            {(() => {
              const Keyboard = settingsTabPages.keyboard
              return <Keyboard />
            })()}
          </Provider>,
        ),
      )
      const keys = [...host.querySelectorAll(".settings-kbd")].map(
        (kbd) => kbd.textContent,
      )
      expect(keys).toContain(sessionList)
      expect(keys).toContain(open)
    })
})

describe("Advanced", () => {
  const navItem = (label: string) =>
    [...document.querySelectorAll<HTMLButtonElement>(".settings-nav-item")].find(
      (item) => item.textContent?.trim() === label,
    )
  const tabNames = () =>
    [...document.querySelectorAll('.settings-tabs [role="tab"]')].map((tab) =>
      tab.textContent?.trim(),
    )

  it("sits in the sidebar just before About, with its icon", async () => {
    await mount()
    await act(async () => setOpen(true))
    const items = [...document.querySelectorAll(".settings-nav-item")].map((item) =>
      item.textContent?.trim(),
    )
    expect(items.slice(-2)).toEqual(["Advanced", "About"])
    expect(navItem("Advanced")?.querySelector("svg.settings-nav-icon")).not.toBeNull()
  })

  it("shows its Experimental tab, with the previews on offer", async () => {
    await mount()
    await act(async () => setOpen(true))
    await act(async () => navItem("Advanced")?.click())
    expect(document.querySelector("#settings-heading")?.textContent).toBe("Advanced")
    expect(document.querySelector(".settings-dek")?.textContent).toBe(
      "Early features you can try before they are finished.",
    )
    // One tab is nothing to choose between: no strip.
    expect(tabNames()).toEqual([])
    // Each preview is a switch, off until turned on.
    const panel = document.querySelector("#settings-panel")
    expect(panel?.textContent).toContain("Subagents")
    expect(panel?.textContent).toContain(
      "A panel of the agents a conversation put to work.",
    )
    expect(panel?.textContent).toContain("Side rail")
    const [subagents, rail] = [
      ...(panel?.querySelectorAll<HTMLButtonElement>('[role="switch"]') ?? []),
    ]
    expect(subagents?.getAttribute("aria-checked")).toBe("true")
    // The side rail is off until turned on.
    expect(rail?.getAttribute("aria-checked")).toBe("false")
    expect(panel?.querySelectorAll("button, input, [role='switch']").length).toBe(2)
    await act(async () => subagents?.click())
    expect(subagents?.getAttribute("aria-checked")).toBe("false")
  })

  it("has taken Experimental out of General", async () => {
    await mount()
    await act(async () => setOpen(true))
    await act(async () => navItem("General")?.click())
    expect(tabNames()).not.toContain("Experimental")
  })
})

describe("what Settings offers", () => {
  it("disables every control a setting not available yet shows, and says so once for its group", async () => {
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
      expect(row?.querySelectorAll("button:not(:disabled)").length, entry.id).toBe(0)
      // Said on the row, or once for its group — never on a row whose group says it.
      const group = row?.closest(".settings-group")
      const said = [...(group?.querySelectorAll(".settings-unavailable") ?? [])]
      // A setting that is a whole card of its own is its group: its note is the group's.
      const onRow =
        row !== group && (row?.textContent?.includes("Not available yet") ?? false)
      expect(said.length + (onRow ? 1 : 0), entry.id).toBe(1)
      // And whoever reads its controls hears why they rest.
      if (said.length === 1)
        for (const control of row?.querySelectorAll<HTMLElement>(
          "[role=switch], [data-slot=segmented-control], .settings-button",
        ) ?? [])
          expect(control.getAttribute("aria-describedby") ?? "", entry.id).toContain(
            said[0].id,
          )
    }
    // A group none of whose settings is available yet says so once, not on every row.
    const general = host
      .querySelector('[data-setting="open-at-login"]')
      ?.closest(".settings-group")
    expect(general?.textContent?.match(/Not available yet/g)).toHaveLength(1)
    // And every other control can be used. A row not available yet is the
    // kit's disabled row (`data-disabled`); a whole card, the app's `data-pending`.
    const available = [
      ...host.querySelectorAll("[data-setting]:not([data-pending], [data-disabled])"),
    ]
    expect(
      available.every((row) => row.querySelectorAll("button:disabled").length === 0),
    ).toBe(true)
  })

  it("never titles a group as its page or its tab is named, so no word shows twice at once", async () => {
    const pages = settingsCategories.flatMap((category) =>
      category.tabs.map((tab) => ({
        category: category.label,
        tab: tab.label,
        id: tab.id,
      })),
    )
    for (const { category, tab, id } of pages) {
      const Page = settingsTabPages[id]
      await act(async () =>
        root.render(
          <Provider store={testStore()}>
            <Page />
          </Provider>,
        ),
      )
      // A title kept for assistive technology only is not on screen.
      const titles = [
        ...host.querySelectorAll('.settings-group:not([data-title="hidden"]) > h2'),
      ].map((title) => title.textContent?.trim())
      expect(titles, id).not.toContain(category)
      expect(titles, id).not.toContain(tab)
    }
  })

  // Classic has no workspace shell for the side rail to stand beside.
  for (const [layout, applies] of [
    ["sidebar", true],
    ["classic", false],
  ] as const)
    it(`${applies ? "enables" : "disables, saying why,"} the side rail's switch in ${layout}`, async () => {
      const kept = new Map<string, string>([["nessa.desktop.workspace-layout", layout]])
      const storage = Object.getOwnPropertyDescriptor(window, "localStorage")
      Object.defineProperty(window, "localStorage", {
        configurable: true,
        value: {
          getItem: (key: string) => kept.get(key) ?? null,
          setItem: (key: string, value: string) => void kept.set(key, value),
          removeItem: (key: string) => void kept.delete(key),
        },
      })
      try {
        const Page = settingsTabPages.experimental
        await act(async () =>
          root.render(
            <Provider store={testStore()}>
              <Page />
            </Provider>,
          ),
        )
        const row = host.querySelector('[data-setting="side-rail"]')
        const toggle = row?.querySelector<HTMLButtonElement>('[role="switch"]')
        expect(toggle?.disabled).toBe(!applies)
        expect(row?.textContent?.includes("Not in Classic")).toBe(!applies)
      } finally {
        if (storage) Object.defineProperty(window, "localStorage", storage)
        else Reflect.deleteProperty(window, "localStorage")
      }
    })

  // The session list is drawn only in three columns: its settings do
  // nothing elsewhere, so there they are disabled and say where they apply.
  const sessionList = ["show-session-list", "running-first"] as const
  for (const [layout, applies] of [
    ["columns", true],
    ["sidebar", false],
    ["classic", false],
  ] as const)
    it(`${applies ? "enables" : "disables, saying where they apply,"} the session list's settings in ${layout}`, async () => {
      // The layout as the window stored it (jsdom here keeps no storage of its own).
      const kept = new Map<string, string>([["nessa.desktop.workspace-layout", layout]])
      const storage = Object.getOwnPropertyDescriptor(window, "localStorage")
      Object.defineProperty(window, "localStorage", {
        configurable: true,
        value: {
          getItem: (key: string) => kept.get(key) ?? null,
          setItem: (key: string, value: string) => void kept.set(key, value),
          removeItem: (key: string) => void kept.delete(key),
        },
      })
      try {
        await act(async () =>
          root.render(
            <Provider store={testStore()}>
              {[settingsTabPages.layout, settingsTabPages.sessions].map((Page, index) => (
                <Page key={index} />
              ))}
            </Provider>,
          ),
        )
        for (const id of sessionList) {
          const row = host.querySelector(`[data-setting="${id}"]`)
          const toggle = row?.querySelector<HTMLButtonElement>('[role="switch"]')
          expect(toggle, id).not.toBeNull()
          expect(toggle?.disabled, id).toBe(!applies)
          expect(row?.textContent?.includes("Three columns only"), id).toBe(!applies)
        }
      } finally {
        if (storage) Object.defineProperty(window, "localStorage", storage)
        else Reflect.deleteProperty(window, "localStorage")
      }
    })
})
