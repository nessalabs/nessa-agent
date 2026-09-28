// @vitest-environment jsdom
/**
 * The two layouts differ only in their sidebar region: each is the one shell
 * with a `SidebarRegion`, so the keyboard, the switcher and the titlebar's
 * controls are the same in both.
 */
import { act } from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { ClockProvider } from "../../adapters/dom/clock"
import { chordEvent } from "../../adapters/dom/shortcuts"
import { loadWorkspace } from "../../adapters/store/commands"
import { settle, testStore } from "../../testing"
import { SessionsInSidebar } from "./sessions-in-sidebar"
import { workspaceShortcuts } from "./shortcuts"
import { ThreeColumns } from "./three-columns"

let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  class Observer {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  Object.assign(globalThis, { ResizeObserver: Observer, IntersectionObserver: Observer })
  Element.prototype.scrollTo ??= () => {}
  Element.prototype.scrollIntoView ??= () => {}
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener() {},
    removeEventListener() {},
  })) as unknown as typeof window.matchMedia
  host = document.createElement("div")
  document.body.append(host)
})

afterEach(() => host.remove())

const press = (command: string) => {
  const binding = workspaceShortcuts.find((each) => each.command === command)
  if (!binding) throw new Error(`no ${command}`)
  window.dispatchEvent(
    new KeyboardEvent("keydown", { ...chordEvent(binding.chord), bubbles: true }),
  )
}

describe("both layouts are the one shell", () => {
  for (const [name, Layout] of [
    ["three columns", ThreeColumns],
    ["sessions in the sidebar", SessionsInSidebar],
  ] as const)
    it(`opens the switcher with ⌘K and ⌘\\, and carries the same titlebar, in ${name}`, async () => {
      const store = testStore()
      await store.dispatch(loadWorkspace())
      await settle()
      const root = createRoot(host)
      await act(async () =>
        root.render(
          <Provider store={store}>
            <ClockProvider now={() => 1000}>
              <Layout hostKind="browser" browserSurface />
            </ClockProvider>
          </Provider>,
        ),
      )
      const titlebar = [...host.querySelectorAll(".workspace-titlebar button")].map(
        (button) => button.getAttribute("aria-label")?.replace(/ \(.*\)$/, ""),
      )
      expect(titlebar.slice(0, 3)).toEqual(["Hide Sidebar", "Go Back", "Go Forward"])
      await act(async () => press("switcher"))
      expect(host.querySelector('[role="dialog"]')?.getAttribute("aria-label")).toBe(
        "Jump to",
      )
      await act(async () => press("switcher"))
      expect(host.querySelector('[role="dialog"]')).toBeNull()
      await act(async () => press("openBeside"))
      expect(host.querySelector('[role="dialog"]')?.getAttribute("aria-label")).toBe(
        "Open beside",
      )
      await act(async () => root.unmount())
    })
})
