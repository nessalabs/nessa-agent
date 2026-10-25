// @vitest-environment jsdom
/**
 * Settings is modal over the window: what lies under it is inert while it is
 * open, so Tab cannot walk out of it into the workspace, and is not after.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { ClockProvider } from "../../clock"
import { openSettings } from "../settings"
import { loadWorkspace } from "../workspace"
import { settle, testStore } from "../workspace/testing"
import { DesktopWindow } from "./desktop-window"

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
    host
      .querySelector<HTMLButtonElement>('.settings [aria-label="Back to nessa Agent"]')
      ?.click(),
  )
  expect(host.querySelector(".settings")).toBeNull()
  expect(under()?.hasAttribute("inert")).toBe(false)
})

it("opens Settings, and keeps the window, when asked for a tab it does not have", async () => {
  // A button wired as `onClick={openSettings}` hands its click event on as the
  // tab: an earlier "nessa Studio" did, and the window went blank.
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
  const event = new MouseEvent("click") as unknown as Parameters<typeof openSettings>[0]
  await act(async () => openSettings(event))
  expect(host.querySelector(".settings")).not.toBeNull()
  expect(
    host.querySelector('.settings [aria-label="Back to nessa Agent"]'),
  ).not.toBeNull()
})
