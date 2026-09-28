// @vitest-environment jsdom
/**
 * The keyboard's ways around the workspace that the pointer has others for:
 * Tab through the panes as they are seen, the context-menu key on a row,
 * and a resize edge that says where it stands.
 */
import { act } from "react"
import { createRoot } from "react-dom/client"
import { describe, expect, it } from "vitest"
import { movePane, singlePane, splitPane } from "../../model/pane-layout"
import { ResizeEdge } from "../../ui/chrome/resize-edge"
import { contextMenuFromKey } from "./context-menu-key"
import { paneTabOrder } from "./pane-tab-order"

// jsdom lays nothing out; every stop here is on screen.
HTMLElement.prototype.getClientRects = () =>
  [new DOMRect(0, 0, 10, 10)] as unknown as DOMRectList

/** A grid whose panes sit in the page in the order they were opened. */
function grid(keys: number[]) {
  const element = document.createElement("div")
  element.innerHTML = keys
    .map(
      (key) =>
        `<article data-pane-key="${key}"><button>Menu ${key}</button><textarea aria-label="Message ${key}"></textarea></article>`,
    )
    .join("")
  const after = document.createElement("button")
  after.textContent = "After"
  document.body.append(element, after)
  return { element, after }
}

/** A stop in the grid, which the test put there. */
const stop = (scope: HTMLElement, selector: string): Element => {
  const found = scope.querySelector(selector)
  if (!found) throw new Error(`no ${selector}`)
  return found
}

const tab = (target: Element, currentTarget: HTMLElement, shiftKey = false) => {
  let prevented = false
  paneTabOrder(layout)({
    key: "Tab",
    shiftKey,
    altKey: false,
    metaKey: false,
    ctrlKey: false,
    target,
    currentTarget,
    preventDefault: () => {
      prevented = true
    },
  } as never)
  return prevented
}

// Pane 2 was opened to the right of pane 1, then moved to its left.
let layout = () => movePane(splitPane(singlePane("a"), 1, "right", "b"), 2, 1, "left")

describe("Tab through the panes", () => {
  it("goes from a pane's last stop to the pane on screen after it, not the one opened after it", () => {
    const { element, after } = grid([1, 2])
    const last2 = stop(element, '[data-pane-key="2"] textarea')
    expect(tab(last2, element)).toBe(true)
    expect(document.activeElement?.textContent).toBe("Menu 1")
    // Past the last pane on screen, out of the panes.
    const last1 = stop(element, '[data-pane-key="1"] textarea')
    expect(tab(last1, element)).toBe(true)
    expect(document.activeElement).toBe(after)
    element.remove()
    after.remove()
  })

  it("goes back with Shift from a pane's first stop, and leaves stops inside a pane to the browser", () => {
    const { element, after } = grid([1, 2])
    const first1 = stop(element, '[data-pane-key="1"] button')
    expect(tab(first1, element, true)).toBe(true)
    expect(document.activeElement?.getAttribute("aria-label")).toBe("Message 2")
    expect(tab(first1, element)).toBe(false)
    element.remove()
    after.remove()
    layout = () => movePane(splitPane(singlePane("a"), 1, "right", "b"), 2, 1, "left")
  })
})

describe("the context-menu key on a row", () => {
  it("opens the row's menu with Shift-F10 or the menu key, and nothing else", () => {
    const row = document.createElement("div")
    document.body.append(row)
    const opened: string[] = []
    row.addEventListener("contextmenu", () => opened.push("menu"))
    const press = (key: string, shiftKey = false) =>
      contextMenuFromKey({ key, shiftKey, target: row, preventDefault() {} })
    expect(press("F10", true)).toBe(true)
    expect(press("ContextMenu")).toBe(true)
    expect(press("F10")).toBe(false)
    expect(press("Enter", true)).toBe(false)
    expect(opened).toEqual(["menu", "menu"])
    row.remove()
  })
})

it("says where a focusable resize edge stands, and between what bounds", async () => {
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  await act(async () =>
    root.render(
      <ResizeEdge
        label="Resize Sidebar"
        value={{ now: 240.4, min: 200, max: 320 }}
        onStart={() => {}}
        onMove={() => {}}
      />,
    ),
  )
  const edge = host.querySelector('[role="separator"]')
  expect(
    ["aria-valuenow", "aria-valuemin", "aria-valuemax"].map((name) =>
      edge?.getAttribute(name),
    ),
  ).toEqual(["240", "200", "320"])
  await act(async () => root.unmount())
  host.remove()
})
