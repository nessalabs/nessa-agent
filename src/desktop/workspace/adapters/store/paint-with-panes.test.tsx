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
import { fitToWindow, loadWorkspace } from "./commands"
import { selectChrome } from "./selectors"
import { settle, testStore } from "../../testing"
import { usePaintedWithPanes } from "./paint-with-panes"
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
