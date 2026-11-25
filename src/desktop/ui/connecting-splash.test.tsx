// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { loadWorkspace } from "../workspace/adapters/store/commands"
import { fakeSource, testStore } from "../workspace/testing"
import { ConnectingSplash, useConnecting } from "./connecting-splash"

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  vi.useFakeTimers()
  vi.spyOn(console, "error").mockImplementation(() => {})
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
  vi.useRealTimers()
  vi.restoreAllMocks()
})

function Splash() {
  return <ConnectingSplash connecting={useConnecting()} />
}

const splash = () => host.querySelector("[data-nessa-connecting]")

it("covers the window until the gateway answers, then fades away", async () => {
  const source = fakeSource()
  source.hold("index")
  const store = testStore(source)
  const loading = store.dispatch(loadWorkspace())
  await act(async () =>
    root.render(
      <Provider store={store}>
        <Splash />
      </Provider>,
    ),
  )
  expect(splash()?.textContent).toContain("Connecting")
  expect(splash()?.hasAttribute("data-leaving")).toBe(false)
  await act(async () => {
    await source.release("index")
    await loading
  })
  expect(splash()?.hasAttribute("data-leaving")).toBe(true)
  await act(async () => vi.advanceTimersByTime(200))
  expect(splash()).toBeNull()
})

it("stays up while the host says the gateway is still starting", async () => {
  const source = fakeSource()
  source.refuse("index", "still-starting")
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  await act(async () =>
    root.render(
      <Provider store={store}>
        <Splash />
      </Provider>,
    ),
  )
  expect(splash()).not.toBeNull()
})

it("gives way to the startup screen for good once startup failed", async () => {
  const source = fakeSource()
  source.refuse("index", "not-listening")
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  await act(async () =>
    root.render(
      <Provider store={store}>
        <Splash />
      </Provider>,
    ),
  )
  expect(splash()).toBeNull()
  // A retry reads again; the startup screen, not the splash, keeps the window.
  source.hold("index")
  const again = store.dispatch(loadWorkspace())
  await act(async () => {})
  expect(store.getState().workspace.status).toBe("loading")
  expect(splash()).toBeNull()
  await act(async () => {
    await source.release("index")
    await again
  })
})
