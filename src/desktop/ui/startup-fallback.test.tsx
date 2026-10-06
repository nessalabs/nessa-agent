// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { loadWorkspace } from "../workspace/adapters/store/commands"
import { fakeSource, testStore } from "../workspace/testing"
import { startupFailureCode } from "../workspace/ui/startup-failure"
import { StartupFallback } from "./startup-fallback"

vi.mock("../../host", () => ({
  restartNessa: vi.fn(),
  quitNessa: vi.fn(),
}))

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  vi.spyOn(console, "error").mockImplementation(() => {})
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})

afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
  vi.restoreAllMocks()
})

it("covers a quiet server with the line and STARTUP_GATEWAY, and logs the cause", async () => {
  const { restartNessa, quitNessa } = await import("../../host")
  const source = fakeSource()
  source.refuse("index", "not-listening")
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  await act(async () =>
    root.render(
      <Provider store={store}>
        <StartupFallback />
      </Provider>,
    ),
  )
  expect(host.querySelector("[data-nessa-startup-line]")?.textContent).toBe(
    "Nessa couldn’t start",
  )
  expect(host.querySelector("[data-nessa-startup-code]")?.textContent).toBe(
    "STARTUP_GATEWAY",
  )
  expect(host.querySelector("[data-nessa-startup-mark]")).toBeNull()
  expect(host.innerHTML).not.toContain("nessa-avatar")
  expect(host.textContent).not.toContain("not answering")
  const logged = vi
    .mocked(console.error)
    .mock.calls.map((call) => call.map(String).join(" "))
    .join("\n")
  expect(logged).toContain("[nessa]")
  expect(logged).toContain("not answering")
  await act(async () =>
    host.querySelector<HTMLButtonElement>("[aria-label=Restart]")?.click(),
  )
  expect(restartNessa).toHaveBeenCalledOnce()
  await act(async () =>
    host.querySelector<HTMLButtonElement>("[aria-label=Quit]")?.click(),
  )
  expect(quitNessa).toHaveBeenCalledOnce()
})

it("stays off a window that is only signed out", async () => {
  const source = fakeSource()
  source.refuse("index", "signed-out")
  const store = testStore(source)
  await store.dispatch(loadWorkspace())
  await act(async () =>
    root.render(
      <Provider store={store}>
        <StartupFallback />
      </Provider>,
    ),
  )
  expect(host.querySelector("[data-nessa-startup-screen]")).toBeNull()
})

it("gives each startup reason its code", () => {
  expect(startupFailureCode("not-started")).toBe("STARTUP_GATEWAY")
  expect(startupFailureCode("not-ready")).toBe("STARTUP_GATEWAY")
  expect(startupFailureCode("not-listening")).toBe("STARTUP_GATEWAY")
  expect(startupFailureCode("wrong-stage")).toBe("STARTUP_STAGE")
  expect(startupFailureCode("signed-out")).toBeNull()
  expect(startupFailureCode("unavailable")).toBeNull()
  expect(startupFailureCode(null)).toBeNull()
})
