// @vitest-environment jsdom
/**
 * A conversation pane carries a sliver of the header — the night scene held
 * still, or the picture — which moves only in the focused pane, and which
 * Settings › Appearance › Header turns off. A new session's home has the
 * header itself, so no sliver.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { loadWorkspace, newSession, openBeside } from "../../adapters/store/commands"
import { selectFocusedPaneKey, selectPlacements } from "../../adapters/store/selectors"
import { useHeaderImage } from "../../../adapters/header-image"
import { ClockProvider } from "../../adapters/dom/clock"
import { fakeSource, settle, testStore } from "../../testing"
import { Pane } from "./pane"

class Observer {
  observe() {}
  unobserve() {}
  disconnect() {}
}

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
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
  window.dispatchEvent(
    new CustomEvent("nessa:desktop-picture-in-conversations", { detail: "on" }),
  )
})

async function panes(withHome = false) {
  const store = testStore(fakeSource())
  await store.dispatch(loadWorkspace())
  store.dispatch(openBeside({ sessionId: "c" }))
  if (withHome) store.dispatch(newSession({ beside: "bottom" }))
  await settle()
  const placements = selectPlacements(store.getState()).panes
  await act(async () =>
    root.render(
      <Provider store={store}>
        <ClockProvider now={() => 1000}>
          {placements.map((placement) => (
            <Pane key={placement.key} placement={placement} multi />
          ))}
        </ClockProvider>
      </Provider>,
    ),
  )
  await act(async () => new Promise((resolve) => setTimeout(resolve, 50)))
  return store
}

const slivers = () => [...host.querySelectorAll<HTMLElement>("[data-sliver]")]

it("draws the night scene, still, at the top of every conversation pane", async () => {
  await panes()
  expect(slivers()).toHaveLength(2)
  for (const sliver of slivers()) {
    expect(sliver.getAttribute("aria-hidden")).toBe("true")
    expect(sliver.querySelector(".desktop-night-scene")?.hasAttribute("data-still")).toBe(
      true,
    )
  }
})

it("leaves a new session's home to its own header", async () => {
  await panes(true)
  const home = host.querySelector(".workspace-pane-home")?.closest(".workspace-pane")
  expect(home).not.toBeNull()
  expect(home?.querySelector("[data-sliver]")).toBeNull()
})

it("goes when Settings turns the picture in conversations off", async () => {
  await panes()
  await act(async () => {
    window.dispatchEvent(
      new CustomEvent("nessa:desktop-picture-in-conversations", { detail: "off" }),
    )
  })
  expect(slivers()).toHaveLength(0)
})

it("moves a GIF only in the focused pane; the others show its first frame", async () => {
  let made = 0
  URL.createObjectURL = () => `blob:picture-${++made}`
  URL.revokeObjectURL = () => {}
  Object.assign(globalThis, {
    createImageBitmap: async () => ({ width: 4, height: 2, close() {} }),
  })
  HTMLCanvasElement.prototype.getContext = (() => ({ drawImage() {} })) as never
  HTMLCanvasElement.prototype.toBlob = function (done: BlobCallback) {
    done(new Blob(["still"], { type: "image/png" }))
  }
  let choose: (blob: Blob) => void = () => {}
  function Chooser() {
    choose = useHeaderImage()[1]
    return null
  }
  const store = await panes()
  const extra = createRoot(document.createElement("div"))
  await act(async () => extra.render(<Chooser />))
  await act(async () => choose(new Blob(["gif"], { type: "image/gif" })))
  for (let tick = 0; tick < 5; tick++)
    await act(async () => new Promise((resolve) => setTimeout(resolve, 0)))
  const focused = selectFocusedPaneKey(store.getState())
  const pictureOf = (key: number | null) =>
    host.querySelector(`[data-pane-key="${key}"] [data-sliver] img`)?.getAttribute("src")
  const other = selectPlacements(store.getState()).panes.find(
    (placement) => placement.key !== focused,
  )?.key
  expect(pictureOf(focused)).toBe("blob:picture-1")
  expect(pictureOf(other ?? null)).toBe("blob:picture-2")
  await act(async () => extra.unmount())
})
