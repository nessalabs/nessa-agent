// @vitest-environment jsdom
/**
 * Sending a new session's first message from its home (#914). The send
 * writes the message into the store at once, and that write replaces the
 * home and removes its focused field. A browser that still has focus in a
 * field the page is removing retires it in the middle of that layout, which
 * is the jank #914 measured. So the pane lets go of the field before the
 * store sees the message: the field is no longer focused when the write
 * happens. `retireHomeFocus` is tested on its own in `focus.test.tsx`; this
 * holds the pane to calling it, and before the write.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { loadWorkspace, newSession } from "../../adapters/store/commands"
import { paneFrame } from "../../../split-panes/testing"
import { placements as placementsOf } from "../../../split-panes/model/pane-sizing"
import { selectPanes } from "../../adapters/store/selectors"
import { ClockProvider } from "../../../../clock"
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
})

it("lets go of the home's field before the first message is written, not after", async () => {
  const store = testStore(fakeSource())
  await store.dispatch(loadWorkspace())
  store.dispatch(newSession())
  await settle()
  const panes = selectPanes(store.getState())
  const placements = panes ? placementsOf(panes.columns).panes : []
  await act(async () =>
    root.render(
      <Provider store={store}>
        <ClockProvider now={() => 1000}>
          {placements.map((placement) => (
            <Pane
              key={placement.key}
              placement={placement}
              frame={paneFrame(placement)}
              multi={placements.length > 1}
            />
          ))}
        </ClockProvider>
      </Provider>,
    ),
  )
  await act(async () => new Promise((resolve) => setTimeout(resolve, 50)))
  const field = host.querySelector<HTMLTextAreaElement>(
    ".workspace-pane-home textarea[aria-label='Message']",
  )
  if (!field) throw new Error("no home field")
  const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")
  await act(async () => {
    setValue?.set?.call(field, "Hello")
    field.dispatchEvent(new Event("input", { bubbles: true }))
  })
  field.focus()
  expect(document.activeElement).toBe(field)

  // What held focus each time the store changed during the send.
  const focusAtWrite: (Element | null)[] = []
  const unsubscribe = store.subscribe(() => focusAtWrite.push(document.activeElement))
  try {
    await act(async () => {
      field.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }))
    })
    await act(async () => settle())
  } finally {
    unsubscribe()
  }
  // The send happened: the message waits in the outbox.
  expect(JSON.stringify(store.getState().workspace.outbox)).toContain('"text":"Hello"')
  expect(focusAtWrite.length).toBeGreaterThan(0)
  expect(
    focusAtWrite[0],
    "the home field still held focus when the send was written",
  ).not.toBe(field)
})
