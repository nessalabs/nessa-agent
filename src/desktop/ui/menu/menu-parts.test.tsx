// @vitest-environment jsdom
/**
 * The menu parts: one list of items renders in a dropdown or a context menu,
 * both on the window's glass, and a chosen item is marked to blink as its
 * menu closes — unless its handler kept the menu open.
 */
import { act, type ReactNode } from "react"
import { createRoot, type Root } from "react-dom/client"
import { renderToStaticMarkup } from "react-dom/server"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuTrigger,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
  MenuCheckboxItem,
  MenuItem,
  MenuLabel,
  MenuSeparator,
  MenuShortcut,
} from "."

let root: Root
let host: HTMLDivElement

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  // Focus starts somewhere real. Left on an element an earlier test removed,
  // jsdom reports the next focus move as the window blurring, and a Radix
  // menu closes on that.
  host.tabIndex = -1
  host.focus()
  root = createRoot(host)
})

afterEach(() => {
  act(() => root.unmount())
  host.remove()
})

// Not modal: a modal menu locks scrolling through a CommonJS package that loads
// a second React under Node (see `server.deps.inline` in vitest.config.ts).
const render = (tree: ReactNode) => act(() => root.render(tree))
const menu = () => document.querySelector<HTMLElement>('[role="menu"]')
const item = (name: string) =>
  [...document.querySelectorAll<HTMLElement>('[role^="menuitem"]')].find((candidate) =>
    candidate.textContent?.startsWith(name),
  )

/** The same items, written once, as the pane and session menus share theirs. */
function Items({ onOpen, onPin }: { onOpen: () => void; onPin: () => void }) {
  return (
    <>
      <MenuLabel>Session</MenuLabel>
      <MenuItem onSelect={onOpen}>
        Open
        <MenuShortcut>↩</MenuShortcut>
      </MenuItem>
      <MenuSeparator />
      <MenuCheckboxItem
        checked={false}
        onSelect={(event) => {
          event.preventDefault()
          onPin()
        }}
      >
        Pinned
      </MenuCheckboxItem>
    </>
  )
}

it("draws a dropdown on the window's glass and marks the chosen item as it closes", () => {
  const onOpen = vi.fn()
  render(
    <DropdownMenu defaultOpen modal={false}>
      <DropdownMenuTrigger>Menu</DropdownMenuTrigger>
      <DropdownMenuContent>
        <Items onOpen={onOpen} onPin={() => {}} />
      </DropdownMenuContent>
    </DropdownMenu>,
  )
  expect(menu()?.classList.contains("desktop-popover")).toBe(true)
  expect(menu()?.dataset.slot).toBe("dropdown-menu-content")
  const open = item("Open")
  act(() => open?.click())
  expect(onOpen).toHaveBeenCalledOnce()
  expect(open?.hasAttribute("data-chosen")).toBe(true)
})

it("renders the same items in a context menu, and a kept-open item is not chosen", () => {
  const onPin = vi.fn()
  render(
    <ContextMenu modal={false}>
      <ContextMenuTrigger>Row</ContextMenuTrigger>
      <ContextMenuContent className="extra">
        <Items onOpen={() => {}} onPin={onPin} />
      </ContextMenuContent>
    </ContextMenu>,
  )
  const row = [...host.querySelectorAll("span")].find(
    (node) => node.textContent === "Row",
  )
  act(() => {
    row?.dispatchEvent(
      new MouseEvent("contextmenu", {
        bubbles: true,
        cancelable: true,
        clientX: 5,
        clientY: 5,
      }),
    )
  })
  expect(menu()?.dataset.slot).toBe("context-menu-content")
  expect(menu()?.className).toContain("desktop-popover extra")
  expect(item("Open")?.dataset.slot).toBe("context-menu-item")
  const pinned = item("Pinned")
  act(() => pinned?.click())
  expect(onPin).toHaveBeenCalledOnce()
  expect(pinned?.hasAttribute("data-chosen")).toBe(false)
  expect(menu()?.dataset.state).toBe("open")
})

it("refuses a part drawn outside any menu, naming where it belongs", () => {
  vi.spyOn(console, "error").mockImplementation(() => {})
  expect(() => renderToStaticMarkup(<MenuItem>Loose</MenuItem>)).toThrow(
    "MenuItem belongs inside a DropdownMenuContent or ContextMenuContent",
  )
  vi.restoreAllMocks()
})
