// @vitest-environment jsdom
/**
 * Settings is modal over the window: what lies under it is inert while it is
 * open, so Tab cannot walk out of it into the workspace, and is not after.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { openSettings } from "../settings"
import { ClockProvider, loadWorkspace } from "../workspace"
import { settle, testStore } from "../workspace/testing"
import { DesktopWindow } from "./desktop-window"

// The classic shell is not what this is about, and its design-system shell
// needs the app's own build to resolve.
vi.mock("./desktop-app", () => ({ DesktopApp: () => null }))

let root: Root
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

it("makes the window under Settings inert while it is open, and only then", async () => {
  const store = testStore()
  await store.dispatch(loadWorkspace())
  await settle()
  await act(async () =>
    root.render(
      <Provider store={store}>
        <ClockProvider now={() => 1000}>
          <DesktopWindow hostKind="browser" browserSurface inspectable={false} />
        </ClockProvider>
      </Provider>,
    ),
  )
  const under = () => host.querySelector(".desktop-window-under")
  expect(under()?.hasAttribute("inert")).toBe(false)
  await act(async () => openSettings())
  expect(host.querySelector(".settings")).not.toBeNull()
  expect(under()?.hasAttribute("inert")).toBe(true)
  await act(async () =>
    host.querySelector<HTMLButtonElement>(".settings-identity")?.click(),
  )
  expect(host.querySelector(".settings")).toBeNull()
  expect(under()?.hasAttribute("inert")).toBe(false)
})
