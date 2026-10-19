// @vitest-environment jsdom
/**
 * The session list draws its search, captions and empty action with the kit
 * (#657). Its rows stay ListRow (#668): a listbox option the kit's
 * SidebarMenuItem is not. `shared-controls.mjs` measures both.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { ClockProvider } from "../../../../clock"
import {
  followWorkspace,
  loadWorkspace,
  renameStarted,
} from "../../adapters/store/commands"
import { selectFocusedSessionId, selectSession } from "../../adapters/store/selectors"
import { fakeSource, settle, testStore } from "../../testing"
import { DeleteSessionHost } from "../session-row-actions"
import { SessionList } from "./session-list"

let host: HTMLDivElement
let root: Root
let unfollow = () => {}

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
  unfollow()
  await act(async () => root.unmount())
  // A menu or dialog open at unmount hands focus back from a timer of its
  // own (see `focusSettled`); it runs here, not in the next test.
  await act(async () => new Promise((resolve) => setTimeout(resolve, 0)))
  host.remove()
  letFocusGo()
})

/**
 * Leaves the document with nothing focused, as a fresh one starts. When the
 * focused element is removed, jsdom keeps the Document itself as the focused
 * area (its focus fixup), and the next `focus()` then fires `blur` at the
 * window: an open Radix menu closes on that, and the next test's menu would
 * close as it opens. Focusing an element and letting it go clears it.
 */
function letFocusGo() {
  const holder = document.createElement("div")
  holder.tabIndex = -1
  document.body.append(holder)
  holder.focus()
  holder.blur()
  holder.remove()
}

async function shown(source = fakeSource()) {
  const store = testStore(source)
  // The source's updates reach the store, as the window follows them.
  unfollow = store.dispatch(followWorkspace())
  await store.dispatch(loadWorkspace())
  await act(async () => {
    root.render(
      <Provider store={store}>
        <ClockProvider now={() => 1_000}>
          <SessionList />
          <DeleteSessionHost />
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
  const caption = host.querySelector("[data-slot=group-header]")
  expect(caption).not.toBeNull()
  expect(caption?.getAttribute("aria-hidden")).toBe("true")
  expect(caption?.closest("[role=group]")?.getAttribute("aria-label")).toMatch(/ \d+$/)
  expect(host.querySelectorAll(".desktop-list-row").length).toBeGreaterThan(0)
  const row = host.querySelector("[data-session-row]")
  expect(row?.getAttribute("role")).toBe("option")
  expect(row?.getAttribute("data-drag-item")).toBeTruthy()
  expect(row?.className).toContain("workspace-session")
  expect(row?.className).toContain("desktop-list-row")
  const selected = host.querySelector("[data-selected=true]")
  expect(selected?.className).toContain("workspace-session")
})

it("an unread session wears StatusGlyph's point", async () => {
  await shown()
  const dot = host.querySelector(".workspace-list [data-status=unread]")
  expect(dot?.tagName).toBe("SPAN")
  expect(dot?.className).toContain("workspace-status")
  expect(dot?.getAttribute("aria-label")).toBe("Unread")
  expect(host.querySelector(".workspace-unread")).toBeNull()
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
  expect(host.querySelector(".desktop-list-row")).toBeNull()
  await act(async () => type(""))
  expect(host.querySelector("[data-slot=empty-state]")).toBeNull()
  expect(host.querySelectorAll(".desktop-list-row").length).toBeGreaterThan(0)
})

/**
 * Waits for where focus lands once a menu or dialog has closed. Radix's focus
 * scope hands focus back (`onCloseAutoFocus`) from a `setTimeout(0)` after
 * unmounting, and `act` ends on a `setImmediate`, which may run before or
 * after it: read at once, focus is wherever the race left it.
 */
async function focusSettled(check: (row: string | null | undefined) => void) {
  await act(async () => {
    await vi.waitFor(() =>
      check(document.activeElement?.getAttribute("data-session-row")),
    )
  })
}

/** The first row that is not the open one, and its id. */
function quietRow() {
  const row = [...host.querySelectorAll<HTMLElement>("[data-session-row]")].find(
    (each) => !each.hasAttribute("data-selected"),
  )
  if (!row) throw new Error("no unselected row")
  return { row, id: row.getAttribute("data-session-row") ?? "" }
}

async function press(element: Element | null) {
  if (!element) throw new Error("nothing to press")
  await act(async () => {
    element.dispatchEvent(
      new MouseEvent("pointerdown", { bubbles: true, cancelable: true, button: 0 }),
    )
    element.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }))
    await settle()
  })
}

/** Opens a row's "More" menu, as the pointer does. */
async function more(row: HTMLElement) {
  const trigger = row.querySelector<HTMLElement>('[data-session-action="more"]')
  if (!trigger) throw new Error("no More button")
  // A pointer press does not focus the button; in jsdom a menu opened from a
  // focused trigger closes as it opens, so focus is let go first.
  if (document.activeElement instanceof HTMLElement) document.activeElement.blur()
  await act(async () => {
    trigger.dispatchEvent(
      new MouseEvent("pointerdown", { bubbles: true, cancelable: true, button: 0 }),
    )
    await settle()
  })
}

const menuWords = () =>
  [...document.querySelectorAll("[role=menuitem]")].map((item) =>
    item.textContent?.trim(),
  )

it("a row's Archive button archives it, and does not open it", async () => {
  const source = fakeSource()
  const store = await shown(source)
  const focused = selectFocusedSessionId(store.getState())
  const { row, id } = quietRow()
  const archive = row.querySelector('[data-session-action="archive"]')
  expect(archive?.getAttribute("aria-label")).toContain("Archive")
  expect(archive?.getAttribute("tabindex")).toBe("-1")
  await press(archive)
  expect(source.calls).toContainEqual(["archive", id, "person"])
  expect(selectFocusedSessionId(store.getState())).toBe(focused)
  expect(host.querySelector(`[data-session-row="${id}"]`)).toBeNull()
})

it("a row's More menu holds what the row can do, less the ways to open it, and Rename edits in place", async () => {
  const source = fakeSource()
  const store = await shown(source)
  const { row, id } = quietRow()
  await more(row)
  expect(menuWords()).toEqual(["Rename", "Pin to Sidebar", "Archive", "Delete…"])
  expect(row.getAttribute("data-actions")).toBe("true")

  // Rename edits the title in place: Escape keeps what was there, ↩ what was typed.
  const before = selectSession(store.getState(), id)?.title
  await press(
    [...document.querySelectorAll("[role=menuitem]")].find(
      (item) => item.textContent === "Rename",
    ) ?? null,
  )
  expect(menuWords()).toEqual([])
  expect(row.hasAttribute("data-actions")).toBe(false)
  expect(field(row)?.value).toBe(before)
  expect(row.querySelector('[data-session-action="more"]')).toBeNull()
  await type(row, "Something else", "Escape")
  expect(field(row)).toBeNull()
  expect(source.calls.some((call) => call[0] === "rename")).toBe(false)
  await act(async () => store.dispatch(renameStarted({ sessionId: id })))
  await type(row, "  Rain   plan  ", "Enter")
  expect(field(row)).toBeNull()
  // What was typed goes to the source, which keeps what its store does of it.
  expect(source.calls).toContainEqual(["rename", id, "  Rain   plan  ", "person"])
  expect(selectSession(store.getState(), id)?.title).toBe("Rain plan")
})

it("a More menu closed on an item that asks nothing more gives focus to its row, not to the pointer's button", async () => {
  const source = fakeSource()
  await shown(source)
  const { row, id } = quietRow()
  await more(row)
  await choose("Pin to Sidebar")
  expect(menuWords()).toEqual([])
  await focusSettled((row) => expect(row).toBe(id))
})

const field = (row: HTMLElement) =>
  row.ownerDocument.querySelector<HTMLInputElement>(
    `[data-session-row="${row.getAttribute("data-session-row")}"] [data-session-rename]`,
  )

async function type(row: HTMLElement, value: string, key?: string) {
  await act(async () => {
    const input = field(row)
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(
      input,
      value,
    )
    input?.dispatchEvent(new Event("input", { bubbles: true }))
    if (key) input?.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }))
    await settle()
  })
}

it("a rename outlives its row being drawn again in another group", async () => {
  const source = fakeSource()
  const store = await shown(source)
  const { row, id } = quietRow()
  await act(async () => store.dispatch(renameStarted({ sessionId: id })))
  await type(row, "Half typed")
  // The agent starts: the session moves to Running, and its row is drawn there afresh.
  const session = selectSession(store.getState(), id)
  if (!session) throw new Error("no session")
  await act(async () => {
    source.emit({
      kind: "session",
      session: { ...session, status: "running", revision: session.revision + 1 },
    })
    await settle()
  })
  const moved = host.querySelector<HTMLElement>(`[data-session-row="${id}"]`)
  expect(moved).not.toBe(row)
  expect(moved?.closest("[role=group]")?.getAttribute("aria-label")).toMatch(/^Running/)
  expect(field(moved as HTMLElement)?.value).toBe("Half typed")
  expect(source.calls.some((call) => call[0] === "rename")).toBe(false)
})

/** Chooses `name` in the open menu. */
async function choose(name: string) {
  await press(
    [...document.querySelectorAll("[role=menuitem]")].find(
      (item) => item.textContent?.trim() === name,
    ) ?? null,
  )
}

/** The row focus is on, if it is one of this list's. */
const focusedHere = () =>
  host.contains(document.activeElement)
    ? document.activeElement?.getAttribute("data-session-row")
    : undefined

it("Delete asks first: Cancel deletes nothing, Delete deletes, and keys in it stay in it", async () => {
  const source = fakeSource()
  const store = await shown(source)
  const { row, id } = quietRow()
  const focused = selectFocusedSessionId(store.getState())
  const dialog = () => document.querySelector("[data-delete-session]")
  await more(row)
  await choose("Delete…")
  expect(dialog()?.textContent).toContain("Delete this session?")
  // ↓ in the question walks nothing in the list behind it.
  await act(async () => {
    dialog()?.dispatchEvent(
      new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }),
    )
    await settle()
  })
  expect(selectFocusedSessionId(store.getState())).toBe(focused)
  const cancel = [...(dialog()?.querySelectorAll("button") ?? [])].find(
    (button) => button.textContent === "Cancel",
  )
  await press(cancel ?? null)
  expect(dialog()).toBeNull()
  expect(source.calls.some((call) => call[0] === "delete")).toBe(false)
  expect(host.querySelector(`[data-session-row="${id}"]`)).not.toBeNull()
  // Focus comes back to the row asked about, where the list's keys work.
  await focusSettled((row) => expect(row).toBe(id))

  await more(row)
  await choose("Delete…")
  await press(dialog()?.querySelector('[data-delete-action="confirm"]') ?? null)
  expect(dialog()).toBeNull()
  expect(source.calls).toContainEqual(["delete", id, "person"])
  expect(host.querySelector(`[data-session-row="${id}"]`)).toBeNull()
  // Deleted, it hands focus to the row beside it.
  await focusSettled((now) => {
    expect(now).toBeTruthy()
    expect(now).not.toBe(id)
  })
})

it("Archive from the More menu hands focus to the row that takes its place", async () => {
  const source = fakeSource()
  await shown(source)
  const { row, id } = quietRow()
  const rows = [...host.querySelectorAll<HTMLElement>("[data-session-row]")]
  const after = rows[rows.indexOf(row) + 1] ?? rows[rows.indexOf(row) - 1]
  const next = after?.getAttribute("data-session-row")
  await more(row)
  await choose("Archive")
  expect(source.calls).toContainEqual(["archive", id, "person"])
  await focusSettled(() => expect(focusedHere()).toBe(next))
})

it("a focused row archived by its button hands focus to the row that takes its place", async () => {
  const source = fakeSource()
  await shown(source)
  const { row, id } = quietRow()
  const rows = [...host.querySelectorAll<HTMLElement>("[data-session-row]")]
  const next = (rows[rows.indexOf(row) + 1] ?? rows[rows.indexOf(row) - 1])?.getAttribute(
    "data-session-row",
  )
  await act(async () => row.focus())
  await press(row.querySelector('[data-session-action="archive"]'))
  expect(source.calls).toContainEqual(["archive", id, "person"])
  await focusSettled(() => expect(focusedHere()).toBe(next))
})

it("focus given back after a delete stays in the list it was asked from, though another list shows the same session", async () => {
  const source = fakeSource()
  await shown(source)
  const { row, id } = quietRow()
  // The sidebar's row for the same session, earlier in the document.
  const elsewhere = document.createElement("button")
  elsewhere.setAttribute("data-session-row", id)
  document.body.prepend(elsewhere)
  try {
    await more(row)
    await choose("Delete…")
    await press(document.querySelector('[data-delete-action="confirm"]'))
    await focusSettled(() => {
      expect(focusedHere()).toBeTruthy()
      expect(focusedHere()).not.toBe(id)
    })
  } finally {
    elsewhere.remove()
  }
})
