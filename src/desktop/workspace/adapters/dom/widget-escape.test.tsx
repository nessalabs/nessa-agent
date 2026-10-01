// @vitest-environment jsdom
/**
 * Escape reaches the widget in front only after ADR 238's owners: a menu or
 * dialog keeps it, as does an Escape already handled (the drag's, the edge
 * peek's, the search's: `use-edge-peek.test.tsx`), the window gone inert
 * under Settings, and the open overview.
 */
import { act, useRef, useState } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { sampleWidgets } from "../../../widgets"
import { loadWorkspace, openWidget, showContent } from "../store/commands"
import { useWorkspaceStore } from "../store/hooks"
import { settle, testStore } from "../../testing"
import { useWidgetEscape, type EscapeScopes } from "./widget-escape"

let root: Root
let host: HTMLDivElement

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

function Window({ inert }: { inert: boolean }) {
  const store = useWorkspaceStore()
  const scope = useRef<HTMLDivElement>(null)
  const [scopes] = useState<EscapeScopes>(() => new Map())
  useWidgetEscape({ store, root: scope, scopes })
  return (
    <div inert={inert || undefined}>
      <div ref={scope}>
        <button type="button">A row</button>
      </div>
    </div>
  )
}

async function page({ inert = false } = {}) {
  const store = testStore()
  await store.dispatch(loadWorkspace())
  await settle()
  store.dispatch(openWidget({ widget: sampleWidgets.trail, place: "window" }))
  await act(async () =>
    root.render(
      <Provider store={store}>
        <Window inert={inert} />
      </Provider>,
    ),
  )
  return store
}

const escape = (handled = false) =>
  act(async () => {
    const event = new KeyboardEvent("keydown", {
      key: "Escape",
      bubbles: true,
      cancelable: true,
    })
    if (handled) event.preventDefault()
    host.querySelector("button")?.dispatchEvent(event)
  })

it("leaves the window from anywhere outside the owners before it", async () => {
  const store = await page()
  await escape()
  expect(store.getState().workspace.content).toBe("panes")
})

it("leaves Escape to a menu or a dialog", async () => {
  const store = await page()
  const menu = document.createElement("div")
  menu.setAttribute("role", "menu")
  const item = document.createElement("button")
  menu.append(item)
  host.querySelector("button")?.after(menu)
  await act(async () =>
    item.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
    ),
  )
  expect(store.getState().workspace.content).toEqual({ widget: sampleWidgets.trail })
  menu.remove()
})

it("leaves an Escape already handled, and one under Settings, alone", async () => {
  const store = await page()
  await escape(true)
  expect(store.getState().workspace.content).toEqual({ widget: sampleWidgets.trail })
  const under = await page({ inert: true })
  await escape()
  expect(under.getState().workspace.content).toEqual({ widget: sampleWidgets.trail })
})

it("leaves an Escape that ends a composition to its field", async () => {
  const store = await page()
  await act(async () =>
    host.querySelector("button")?.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Escape",
        bubbles: true,
        cancelable: true,
        isComposing: true,
      }),
    ),
  )
  expect(store.getState().workspace.content).toEqual({ widget: sampleWidgets.trail })
})

it("leaves the open overview's Escape to it", async () => {
  const store = await page()
  store.dispatch(showContent({ content: "agents" }))
  await escape()
  expect(store.getState().workspace.content).toBe("agents")
})
