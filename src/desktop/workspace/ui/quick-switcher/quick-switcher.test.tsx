// @vitest-environment jsdom
/**
 * A pick in the quick switcher hands the caret to the focused pane's
 * composer, which may still be filling in; the hand-off is called off when
 * the person presses somewhere else first, so the caret never lands after
 * they have moved on.
 */
import { act, useState } from "react"
import { createRoot } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { ClockProvider } from "../../adapters/dom/clock"
import { focusedPaneAttribute } from "../../adapters/dom/focus"
import { loadWorkspace } from "../../adapters/store/commands"
import { settle, testStore } from "../../testing"
import { QuickSwitcher } from "./quick-switcher"

let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  Element.prototype.scrollIntoView ??= () => {}
  host = document.createElement("div")
  document.body.append(host)
})

afterEach(() => host.remove())

const frames = (count: number) =>
  act(async () => {
    for (let frame = 0; frame < count; frame++)
      await new Promise<void>((done) => requestAnimationFrame(() => done()))
  })

/** A composer that appears only a few frames after the pick, as a new pane's does. */
function composerLater() {
  const pane = document.createElement("div")
  pane.setAttribute(focusedPaneAttribute, "")
  pane.innerHTML = '<div class="desktop-composer"><textarea></textarea></div>'
  return pane
}

async function pickAndClose() {
  const store = testStore()
  await store.dispatch(loadWorkspace())
  await settle()
  function Host() {
    const [open, setOpen] = useState(true)
    return open ? (
      <QuickSwitcher
        mode="open"
        channelId="desktop"
        onClose={() => setOpen(false)}
        onPick={() => setOpen(false)}
      />
    ) : null
  }
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <Provider store={store}>
        <ClockProvider now={() => 1000}>
          <Host />
        </ClockProvider>
      </Provider>,
    ),
  )
  const field = host.querySelector("input")
  await act(async () => {
    field?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }))
  })
  return root
}

it("hands the caret to the composer once it is on the page", async () => {
  const root = await pickAndClose()
  await frames(2)
  const pane = composerLater()
  host.append(pane)
  await frames(2)
  expect(document.activeElement).toBe(pane.querySelector("textarea"))
  await act(async () => root.unmount())
})

it("lets go of its watch for a press once the caret has landed", async () => {
  const added: EventListenerOrEventListenerObject[] = []
  const removed: EventListenerOrEventListenerObject[] = []
  const add = window.addEventListener.bind(window)
  const remove = window.removeEventListener.bind(window)
  window.addEventListener = ((
    type: string,
    listener: EventListenerOrEventListenerObject,
    options?: unknown,
  ) => {
    if (type === "pointerdown") added.push(listener)
    add(type, listener, options as AddEventListenerOptions)
  }) as typeof window.addEventListener
  window.removeEventListener = ((
    type: string,
    listener: EventListenerOrEventListenerObject,
    options?: unknown,
  ) => {
    if (type === "pointerdown") removed.push(listener)
    remove(type, listener, options as EventListenerOptions)
  }) as typeof window.removeEventListener
  try {
    const root = await pickAndClose()
    const pane = composerLater()
    host.append(pane)
    await frames(3)
    expect(document.activeElement).toBe(pane.querySelector("textarea"))
    // Every press watch it added, it took away once the caret landed.
    expect(added.length).toBeGreaterThan(0)
    expect(added.every((listener) => removed.includes(listener))).toBe(true)
    await act(async () => root.unmount())
  } finally {
    window.addEventListener = add
    window.removeEventListener = remove
  }
})

it("calls the hand-off off when the person presses somewhere else first", async () => {
  const root = await pickAndClose()
  await frames(2)
  const elsewhere = document.createElement("button")
  host.append(elsewhere)
  elsewhere.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }))
  elsewhere.focus()
  const pane = composerLater()
  host.append(pane)
  await frames(3)
  expect(document.activeElement).toBe(elsewhere)
  await act(async () => root.unmount())
})
