// @vitest-environment jsdom
/**
 * A fold that happens while the window is mounting, before the paint gate
 * exists. The gate has to read that store on the way in, or the window
 * keeps the sidebar it rendered first.
 */
import { act, useLayoutEffect } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { drawn } from "../../../model/side-column"
import { fitToWindow, loadWorkspace, openBeside, toggleSidebar } from "./commands"
import { selectChrome, selectColumnCount } from "./selectors"
import { controlledAnimationFrames, settle, testStore } from "../../testing"
import { listPaintFrame, paneBoxFrame, usePaintedWithPanes } from "./paint-with-panes"
import { useWorkspaceDispatch } from "./hooks"

let host: HTMLDivElement
let root: Root

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

function FoldOnMount() {
  const dispatch = useWorkspaceDispatch()
  useLayoutEffect(() => {
    dispatch(fitToWindow({ windowWidth: 600, sidebarWidth: 240, sessionListWidth: 312 }))
  }, [dispatch])
  return null
}

function Painted() {
  const chrome = usePaintedWithPanes(selectChrome)
  return (
    <>
      <output data-sidebar={drawn(chrome.sidebar) ? "open" : "closed"} />
      <FoldOnMount />
    </>
  )
}

it("paints a fold that lands before the gate exists", async () => {
  const store = testStore()
  await store.dispatch(loadWorkspace())
  await settle()
  expect(drawn(store.getState().workspace.chrome.sidebar)).toBe(true)
  await act(async () => {
    root.render(
      <Provider store={store}>
        <Painted />
      </Provider>,
    )
  })
  expect(drawn(store.getState().workspace.chrome.sidebar)).toBe(false)
  expect(host.querySelector("output")?.getAttribute("data-sidebar")).toBe("closed")
})

function PaintedChrome({ frames, mark }: { frames: number; mark: string }) {
  const chrome = usePaintedWithPanes(selectChrome, frames)
  return (
    <output data-mark={mark} data-sidebar={drawn(chrome.sidebar) ? "open" : "closed"} />
  )
}

const painted = (mark: string) =>
  host.querySelector(`output[data-mark="${mark}"]`)?.getAttribute("data-sidebar")

it("forwards a sidebar toggle to listeners that already painted the column change", async () => {
  const store = testStore()
  await store.dispatch(loadWorkspace())
  await settle()
  const frames = controlledAnimationFrames()
  const open = () =>
    drawn(store.getState().workspace.chrome.sidebar) ? "open" : "closed"
  try {
    await act(async () => {
      root.render(
        <Provider store={store}>
          <PaintedChrome frames={paneBoxFrame} mark="box" />
          <PaintedChrome frames={listPaintFrame} mark="late" />
        </Provider>,
      )
    })
    const columns = selectColumnCount(store.getState())
    await act(async () => {
      store.dispatch(openBeside({ sessionId: "b" }))
    })
    expect(selectColumnCount(store.getState())).toBeGreaterThan(columns)
    expect(painted("box")).toBe("open")
    await act(async () => frames.runFrame())
    await act(async () => frames.runFrame())
    await act(async () => frames.runFrame())
    const shown = painted("box")
    expect(painted("late")).toBe("open")
    await act(async () => {
      store.dispatch(toggleSidebar())
    })
    expect(open()).not.toBe(shown)
    expect(painted("box")).toBe(open())
    expect(painted("late")).toBe(shown)
    await act(async () => frames.runFrame())
    await act(async () => frames.runFrame())
    expect(painted("late")).toBe(open())
  } finally {
    frames.restore()
  }
})
