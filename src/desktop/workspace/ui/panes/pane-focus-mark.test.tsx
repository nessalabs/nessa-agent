// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { focusPane, loadWorkspace, newSession } from "../../adapters/store/commands"
import { controlledAnimationFrames, settle, testStore } from "../../testing"
import { FocusedSliver, PaneFocusMark } from "./pane-focus-mark"

vi.mock("../../../ui/header-art", () => ({
  HeaderSliver: ({ moving }: { moving: boolean }) => <span data-moving={moving} />,
}))

function Page() {
  return (
    <article data-pane-key="2">
      <PaneFocusMark paneKey={2} multi />
      <FocusedSliver paneKey={2} />
    </article>
  )
}

let root: Root
let host: HTMLDivElement
let frames: ReturnType<typeof controlledAnimationFrames>
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })
  frames = controlledAnimationFrames()
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})
afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
  frames.restore()
})
async function twoPanes() {
  const store = testStore()
  await store.dispatch(loadWorkspace())
  store.dispatch(newSession({ beside: "right" }))
  store.dispatch(focusPane({ pane: 1 }))
  await settle()
  return store
}
const marked = () => host.querySelector("article")?.hasAttribute("data-pane-focused")
const moving = () => host.querySelector("[data-moving]")?.getAttribute("data-moving")

it("applies the mounted owner's focus and sliver on its requested frame", async () => {
  const store = await twoPanes()
  await act(async () =>
    root.render(
      <Provider store={store}>
        <Page />
      </Provider>,
    ),
  )
  expect(marked()).toBe(false)
  expect(moving()).toBe("false")
  await act(async () => {
    store.dispatch(focusPane({ pane: 2 }))
  })
  expect(marked()).toBe(false)
  await act(async () => frames.runFrame())
  expect(marked()).toBe(true)
  expect(moving()).toBe("true")
  await act(async () => {
    store.dispatch(focusPane({ pane: 1 }))
  })
  await act(async () => frames.runFrame())
  expect(marked()).toBe(false)
  expect(moving()).toBe("false")
})

it.each(["store", "layout"])(
  "a retired %s cannot change its replacement's focus or sliver",
  async (cause) => {
    const previous = await twoPanes()
    await act(async () =>
      root.render(
        <Provider store={previous}>
          <Page key="old" />
        </Provider>,
      ),
    )
    const request = window.requestAnimationFrame
    const captured: FrameRequestCallback[] = []
    window.requestAnimationFrame = (callback) => {
      captured.push(callback)
      return request(callback)
    }
    try {
      await act(async () => {
        previous.dispatch(focusPane({ pane: 2 }))
      })
      expect(captured).toHaveLength(2)
      const retired = [...captured]
      const replacement = await twoPanes()
      await act(async () =>
        root.render(
          <Provider store={replacement}>
            <Page key={cause === "layout" ? "new" : "old"} />
          </Provider>,
        ),
      )
      expect(marked()).toBe(false)
      expect(frames.pending()).toBe(0)
      await act(async () => {
        for (const callback of retired) callback(performance.now())
      })
      expect(frames.pending()).toBe(0)
      expect(marked()).toBe(false)
      expect(moving()).toBe("false")
      await act(async () => {
        replacement.dispatch(focusPane({ pane: 2 }))
      })
      await act(async () => frames.runFrame())
      expect(marked()).toBe(true)
      expect(moving()).toBe("true")
      await act(async () => {
        for (const callback of retired) callback(performance.now())
      })
      expect(marked()).toBe(true)
      expect(moving()).toBe("true")
    } finally {
      window.requestAnimationFrame = request
    }
  },
)
