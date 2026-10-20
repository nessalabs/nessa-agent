// @vitest-environment jsdom
import { act, useRef, useState } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { focusSessionRow, useRowFocusKept } from "./row-focus"

let host: HTMLDivElement
let root: Root
let remove: (id: string) => void = () => {}

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

/** Rows, each with a control drawn beside it, as a swipe tray's buttons are. */
function List() {
  const list = useRef<HTMLUListElement>(null)
  useRowFocusKept(list, "[data-session-row]")
  const [ids, setIds] = useState(["a", "b", "c"])
  remove = (id) => setIds((now) => now.filter((each) => each !== id))
  return (
    <ul ref={list}>
      {ids.map((id) => (
        <li key={id}>
          <button type="button" data-session-row={id}>
            {id}
          </button>
          <div>
            <button type="button" data-beside={id}>
              Archive
            </button>
          </div>
        </li>
      ))}
    </ul>
  )
}

it("focuses the session's row as the list draws it now", async () => {
  await act(async () => root.render(<List />))
  expect(focusSessionRow("b")).toBe(true)
  expect(document.activeElement?.getAttribute("data-session-row")).toBe("b")
  expect(focusSessionRow("missing")).toBe(false)
})

it("a row whose control beside it held focus hands focus to the row in its place", async () => {
  await act(async () => root.render(<List />))
  // An earlier row held focus first; the control then takes it.
  await act(async () =>
    host.querySelector<HTMLElement>('[data-session-row="a"]')?.focus(),
  )
  await act(async () => host.querySelector<HTMLElement>('[data-beside="b"]')?.focus())
  await act(async () => remove("b"))
  expect(document.activeElement?.getAttribute("data-session-row")).toBe("c")
})
