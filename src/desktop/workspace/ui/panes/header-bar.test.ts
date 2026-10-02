import { expect, it } from "vitest"
import { headerBar } from "./header-bar"

it("moves the window when alone in the row: one pane, or the window over the panes", () => {
  expect(headerBar({ pane: 1, multi: false })).toEqual({
    bar: {
      "data-split-keeps": "top-left",
      "data-tauri-drag-region": true,
      "data-drag-pane": undefined,
    },
    spacer: { "data-tauri-drag-region": true },
  })
  expect(headerBar({ pane: null, multi: true })).toEqual({
    bar: {
      "data-split-keeps": undefined,
      "data-tauri-drag-region": true,
      "data-drag-pane": undefined,
    },
    spacer: { "data-tauri-drag-region": true },
  })
})

it("carries its pane beside others, and moves no window", () => {
  expect(headerBar({ pane: 3, multi: true })).toEqual({
    bar: {
      "data-split-keeps": "top-left",
      "data-tauri-drag-region": undefined,
      "data-drag-pane": 3,
    },
    spacer: { "data-tauri-drag-region": undefined },
  })
})
