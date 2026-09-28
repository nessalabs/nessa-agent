// @vitest-environment jsdom
/**
 * The webview's own right-click menu: kept off the window's chrome, left on
 * editable and selected text, and one ⌥ away in a development build.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { useWebviewMenu } from "./use-webview-menu"

function WebviewMenu({ inspectable }: { inspectable: boolean }) {
  useWebviewMenu({ inspectable })
  return null
}

let root: Root
let host: HTMLDivElement

function mount(inspectable: boolean) {
  root = createRoot(document.createElement("div"))
  act(() => root.render(<WebviewMenu inspectable={inspectable} />))
}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  host.innerHTML = `
    <div class="chrome">Sessions</div>
    <textarea></textarea>
    <input type="text" />
    <input type="checkbox" />
    <div contenteditable="true"><p class="written">Draft</p><span contenteditable="false" class="chip">@file</span></div>
    <p class="transcript">Some words</p>`
  document.body.append(host)
})

afterEach(() => {
  act(() => root.unmount())
  host.remove()
  document.getSelection()?.removeAllRanges()
})

/** Right-clicks the element; true when the webview's menu would open. */
function rightClick(selector: string, init: MouseEventInit = {}): boolean {
  const target = host.querySelector(selector)
  if (!target) throw new Error(`no ${selector}`)
  const event = new MouseEvent("contextmenu", {
    bubbles: true,
    cancelable: true,
    ...init,
  })
  target.dispatchEvent(event)
  return !event.defaultPrevented
}

it("opens nothing on the window's chrome, or on a control that is an input", () => {
  mount(false)
  expect(rightClick(".chrome")).toBe(false)
  expect(rightClick("input[type=checkbox]")).toBe(false)
})

it("leaves the text menu in fields and editable text, but not on an uneditable chip inside it", () => {
  mount(false)
  expect(rightClick("textarea")).toBe(true)
  expect(rightClick("input[type=text]")).toBe(true)
  expect(rightClick(".written")).toBe(true)
  expect(rightClick(".chip")).toBe(false)
})

it("leaves the text menu on selected text, and only on it", () => {
  mount(false)
  const range = document.createRange()
  range.selectNodeContents(host.querySelector(".transcript") as Node)
  document.getSelection()?.addRange(range)
  const rect = { left: 10, top: 10, right: 90, bottom: 26 } as DOMRect
  // jsdom lays nothing out; the selection is drawn where this says.
  const layout = Range.prototype.getClientRects
  Range.prototype.getClientRects = () => [rect] as unknown as DOMRectList
  try {
    expect(rightClick(".transcript", { clientX: 40, clientY: 18 })).toBe(true)
    expect(rightClick(".transcript", { clientX: 200, clientY: 18 })).toBe(false)
  } finally {
    Range.prototype.getClientRects = layout
  }
})

it("reaches the webview's menu with ⌥ held only in a development build", () => {
  mount(true)
  expect(rightClick(".chrome", { altKey: true })).toBe(true)
  expect(rightClick(".chrome")).toBe(false)
  act(() => root.unmount())
  mount(false)
  expect(rightClick(".chrome", { altKey: true })).toBe(false)
})
