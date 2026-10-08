// @vitest-environment jsdom
/**
 * The session list draws its repeated pieces with the kit (#657): the search
 * field, the group captions, the rows, the unread mark, and the empty state.
 * `shared-controls.mjs` measures them in the window.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it } from "vitest"
import { ClockProvider } from "../../adapters/dom/clock"
import { loadWorkspace } from "../../adapters/store/commands"
import { settle, testStore } from "../../testing"
import { SessionList } from "./session-list"

let host: HTMLDivElement
let root: Root

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
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

async function shown() {
  const store = testStore()
  await store.dispatch(loadWorkspace())
  await act(async () => {
    root.render(
      <Provider store={store}>
        <ClockProvider now={() => 1_000}>
          <SessionList />
        </ClockProvider>
      </Provider>,
    )
  })
  await act(async () => {
    await settle()
  })
  return store
}

it("draws the search, the captions and the rows with the kit", async () => {
  await shown()
  expect(host.querySelector("[data-slot=search-field]")).not.toBeNull()
  expect(host.querySelector("[data-slot=search-field] input")?.getAttribute("type")).toBe(
    "search",
  )
  expect(host.querySelectorAll("[data-slot=group-header]").length).toBeGreaterThan(0)
  expect(host.querySelectorAll("[data-slot=sidebar-menu-item]").length).toBeGreaterThan(0)
  const row = host.querySelector("[data-session-row]")
  expect(row?.getAttribute("role")).toBe("option")
  expect(row?.getAttribute("data-drag-item")).toBeTruthy()
  // The menu trigger merges onto the kit row; the kit's own classes stay.
  expect(row?.className).toContain("workspace-session")
  expect(row?.className).toContain("rounded-lg")
  const selected = host.querySelector("[data-selected=true]")
  expect(selected?.closest("li")?.className).toContain("workspace-session-selected")
})

it("an unread session wears the kit's Badge", async () => {
  await shown()
  const dot = host.querySelector(".workspace-unread")
  expect(dot?.tagName).toBe("SPAN")
  expect(dot?.className).toContain("workspace-unread")
  expect(dot?.className).toContain("rounded-full")
  expect(dot?.getAttribute("aria-label")).toBe("Unread")
})

it("a query that matches nothing shows the kit's empty state, and clearing it brings the rows back", async () => {
  await shown()
  const input = host.querySelector<HTMLInputElement>("[data-slot=search-field] input")
  expect(input).not.toBeNull()
  const type = (value: string) => {
    const setter = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set
    setter?.call(input, value)
    input?.dispatchEvent(new Event("input", { bubbles: true }))
  }
  await act(async () => type("zzzz-no-such-session"))
  expect(host.querySelector("[data-slot=empty-state]")?.textContent).toContain(
    "No sessions match.",
  )
  expect(host.querySelector("[data-slot=sidebar-menu-item]")).toBeNull()
  await act(async () => type(""))
  expect(host.querySelector("[data-slot=empty-state]")).toBeNull()
  expect(host.querySelectorAll("[data-slot=sidebar-menu-item]").length).toBeGreaterThan(0)
})
