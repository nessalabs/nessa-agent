// @vitest-environment jsdom
/**
 * A selector paints from the fields it reads. A session's own text updates
 * in the turn. A new pane column renders with the new boxes. Opening a
 * pane does not call a selector that reads neither the panes nor the
 * chrome (`selector-paint.ts`).
 */
import { act } from "react"
import { createRoot } from "react-dom/client"
import { Provider, shallowEqual } from "react-redux"
import { beforeEach, expect, it } from "vitest"
import { panesOf } from "../../../split-panes/model/pane-layout"
import { focusPane, loadWorkspace, openBeside, setComposerText } from "./commands"
import { useWorkspaceSelector } from "./hooks"
import { paneBoxFrame } from "./paint-with-panes"
import { selectColumnCount, selectComposerText, selectFocusedPaneKey } from "./selectors"
import { controlledAnimationFrames, testStore } from "../../testing"

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
})

async function shown() {
  const store = testStore()
  await act(async () => {
    await store.dispatch(loadWorkspace())
  })
  const frames = controlledAnimationFrames()
  const host = document.createElement("div")
  const root = createRoot(host)
  let textReads = 0
  function Text() {
    const text = useWorkspaceSelector((state) => {
      textReads += 1
      return selectComposerText(state, "a")
    })
    return <p data-text={text} />
  }
  function Columns() {
    const columns = useWorkspaceSelector(selectColumnCount)
    const focused = useWorkspaceSelector(selectFocusedPaneKey)
    // A new object every read. Shallow equality must keep the snapshot, or this render does not settle.
    useWorkspaceSelector((state) => ({ columns: selectColumnCount(state) }), shallowEqual)
    return <p data-columns={columns} data-focused={focused ?? ""} />
  }
  await act(async () => {
    root.render(
      <Provider store={store}>
        <Text />
        <Columns />
      </Provider>,
    )
  })
  return {
    store,
    frames,
    host,
    textReads: () => textReads,
    unmount: () => act(() => root.unmount()),
  }
}

it("paints a changed field in the turn, and does not call a selector whose fields stayed", async () => {
  const { store, frames, host, textReads, unmount } = await shown()
  const text = () => host.querySelector("[data-text]")
  const columns = () => host.querySelector("[data-columns]")
  await act(async () => {
    store.dispatch(setComposerText({ sessionId: "a", text: "kept" }))
  })
  expect(text()?.getAttribute("data-text")).toBe("kept")

  const before = textReads()
  const count = columns()?.getAttribute("data-columns")
  await act(async () => {
    store.dispatch(openBeside({ sessionId: "c" }))
  })
  expect(textReads()).toBe(before)
  expect(columns()?.getAttribute("data-columns")).toBe(count)
  for (let frame = 1; frame < paneBoxFrame; frame++) {
    await act(() => frames.runFrame())
    expect(columns()?.getAttribute("data-columns")).toBe(count)
  }
  await act(() => frames.runFrame())
  expect(columns()?.getAttribute("data-columns")).not.toBe(count)

  const layout = store.getState().workspace.panes
  const other = layout
    ? panesOf(layout).find(
        (pane) => String(pane.key) !== columns()?.getAttribute("data-focused"),
      )
    : undefined
  expect(other).toBeTruthy()
  await act(async () => {
    if (other) store.dispatch(focusPane({ pane: other.key }))
  })
  expect(columns()?.getAttribute("data-focused")).toBe(String(other?.key))

  await unmount()
  frames.restore()
})
