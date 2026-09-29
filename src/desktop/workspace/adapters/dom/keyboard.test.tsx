// @vitest-environment jsdom
/**
 * The keyboard's ways around the workspace that the pointer has others for:
 * the context-menu key on a row. Tab through the panes is the split panes'
 * own (`split-panes/adapters/dom/tab-order.test.tsx`), and a resize edge's
 * standing the desktop's (`src/desktop/ui/resize-edge.test.tsx`).
 */
import { describe, expect, it } from "vitest"
import { contextMenuFromKey } from "./context-menu-key"

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
