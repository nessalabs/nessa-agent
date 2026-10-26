// @vitest-environment jsdom
/**
 * The overview's filter menu: each choice shows its number, and pressing the
 * number picks that choice as a click does, closing the menu; a held
 * modifier or a key that names no choice leaves it alone.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { AgentsFilter } from "../../model/overview/filter"
import { FilterMenu } from "./filter-menu"

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  host.tabIndex = -1
  host.focus()
  root = createRoot(host)
})

afterEach(() => {
  act(() => root.unmount())
  host.remove()
})

const filter: AgentsFilter = { scope: "ongoing", range: "any", tags: [] }
const menu = () => document.querySelector<HTMLElement>('[role="menu"]')
const item = (name: string) =>
  [...document.querySelectorAll<HTMLElement>('[role="menuitemradio"]')].find(
    (candidate) => candidate.textContent?.startsWith(name),
  )

function opened(onChange: (filter: AgentsFilter) => void) {
  act(() => root.render(<FilterMenu filter={filter} onChange={onChange} />))
  const chip = host.querySelector<HTMLButtonElement>(".agents-filter")
  act(() => {
    chip?.focus()
    chip?.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }))
  })
}

const press = (key: string, init: KeyboardEventInit = {}) =>
  act(() => {
    ;(document.activeElement ?? menu())?.dispatchEvent(
      new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init }),
    )
  })

it("names each choice's number to assistive technology, not in its name", () => {
  opened(() => {})
  expect(item("Last 7 days")?.getAttribute("aria-keyshortcuts")).toBe("5")
  expect(
    item("Last 7 days")
      ?.querySelector('[data-slot$="menu-shortcut"]')
      ?.getAttribute("aria-hidden"),
  ).toBe("true")
})

it("picks the choice a number names by choosing its row, and closes", () => {
  const onChange = vi.fn()
  opened(onChange)
  const week = item("Last 7 days")
  press("5")
  // Chosen as a click chooses it: marked to blink as the menu fades.
  expect(week?.hasAttribute("data-chosen")).toBe(true)
  expect(onChange).toHaveBeenCalledWith({ ...filter, range: "week" })
  expect(menu()).toBeNull()
})

it("leaves a number with a modifier, and a key that names no choice, alone", () => {
  const onChange = vi.fn()
  opened(onChange)
  press("5", { metaKey: true })
  press("9")
  expect(onChange).not.toHaveBeenCalled()
  expect(menu()).not.toBeNull()
})
