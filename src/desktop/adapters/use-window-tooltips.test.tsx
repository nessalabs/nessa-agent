// @vitest-environment jsdom
/**
 * The window's one tooltip: shown after a rest on a control that asks for
 * one, drawn inside that control's surface with its chord as a key, and gone
 * on a press or Escape.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { useWindowTooltips } from "./use-window-tooltips"

function Tooltips() {
  useWindowTooltips()
  return null
}

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  vi.useFakeTimers()
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  host.innerHTML = `<div data-surface="window" data-desktop-theme="ember">
      <button aria-label="Close Pane (⌘W)" data-tooltip="Close Pane" data-tooltip-shortcut="⌘W">×</button>
      <button aria-label="Add channel" data-tooltip="Adding channels isn’t available yet">+</button>
    </div>`
  document.body.append(host)
  root = createRoot(document.createElement("div"))
  act(() => root.render(<Tooltips />))
})

afterEach(() => {
  act(() => root.unmount())
  host.remove()
  vi.useRealTimers()
})

const tooltip = () => document.querySelector<HTMLElement>(".desktop-tooltip")
const hover = (target: Element) =>
  target.dispatchEvent(
    new PointerEvent("pointerover", { bubbles: true, pointerType: "mouse" }),
  )

it("shows a control's words and chord in its surface after a rest, and hides on a press", () => {
  const [close] = host.querySelectorAll("button")
  hover(close)
  expect(tooltip()?.hidden ?? true).toBe(true)
  act(() => vi.advanceTimersByTime(600))
  expect(tooltip()?.hidden).toBe(false)
  expect(tooltip()?.querySelector("span")?.textContent).toBe("Close Pane")
  expect(tooltip()?.querySelector(".desktop-tooltip-shortcut")?.textContent).toBe("⌘W")
  expect(tooltip()?.closest("[data-surface]")?.getAttribute("data-desktop-theme")).toBe(
    "ember",
  )
  // Its words are already the control's name: nothing more to describe.
  expect(close.hasAttribute("aria-describedby")).toBe(false)
  close.dispatchEvent(new PointerEvent("pointerdown", { bubbles: true }))
  expect(tooltip()?.hidden).toBe(true)
  // Pressed, it shows nothing more until the pointer has left it.
  hover(close)
  act(() => vi.advanceTimersByTime(600))
  expect(tooltip()?.hidden).toBe(true)
})

it("shows nothing for a control whose menu is open, or while a button is held", () => {
  const [close, add] = host.querySelectorAll("button")
  close.setAttribute("data-state", "open")
  hover(close)
  act(() => vi.advanceTimersByTime(600))
  expect(tooltip()?.hidden ?? true).toBe(true)
  add.dispatchEvent(
    new PointerEvent("pointerover", { bubbles: true, pointerType: "mouse", buttons: 1 }),
  )
  act(() => vi.advanceTimersByTime(600))
  expect(tooltip()?.hidden ?? true).toBe(true)
})

it("describes a control whose words say more than its name, and hides on Escape", () => {
  const [, add] = host.querySelectorAll("button")
  hover(add)
  act(() => vi.advanceTimersByTime(600))
  expect(tooltip()?.querySelector<HTMLElement>(".desktop-tooltip-shortcut")?.hidden).toBe(
    true,
  )
  expect(add.getAttribute("aria-describedby")).toBe(tooltip()?.id)
  window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))
  expect(tooltip()?.hidden).toBe(true)
  expect(add.hasAttribute("aria-describedby")).toBe(false)
})

it("shows the next at once while one has only just hidden", () => {
  const [close, add] = host.querySelectorAll("button")
  hover(close)
  act(() => vi.advanceTimersByTime(600))
  hover(add)
  act(() => vi.advanceTimersByTime(0))
  expect(tooltip()?.querySelector("span")?.textContent).toBe(
    "Adding channels isn’t available yet",
  )
})
