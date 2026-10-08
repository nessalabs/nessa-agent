// @vitest-environment jsdom
import { renderToStaticMarkup } from "react-dom/server"
import { expect, it } from "vitest"
import { PaneHeaderFrame } from "./pane-header-frame"

const header = (node: React.ReactElement) => {
  const host = document.createElement("div")
  host.innerHTML = renderToStaticMarkup(node)
  return host.querySelector("header") as HTMLElement
}

it("lays every header out as one bar: its name, the spacer, what is added, then its actions", () => {
  const bar = header(
    <PaneHeaderFrame
      pane={1}
      multi
      name={<span>Title</span>}
      accessories={<span data-added />}
      actions={<button type="button">Close</button>}
    />,
  )
  expect([...bar.children].map((child) => child.className || child.tagName)).toEqual([
    "workspace-pane-name",
    "workspace-spacer",
    "SPAN",
    "workspace-pane-actions",
  ])
})

it("carries its pane beside others, and moves the window alone or as the window", () => {
  const beside = header(<PaneHeaderFrame pane={2} multi name="a" actions={null} />)
  expect(beside.dataset.dragPane).toBe("2")
  expect(beside.hasAttribute("data-tauri-drag-region")).toBe(false)
  const alone = header(<PaneHeaderFrame pane={2} multi={false} name="a" actions={null} />)
  expect(alone.hasAttribute("data-drag-pane")).toBe(false)
  expect(alone.hasAttribute("data-tauri-drag-region")).toBe(true)
  expect(
    alone.querySelector(".workspace-spacer")?.hasAttribute("data-tauri-drag-region"),
  ).toBe(true)
  const window = header(<PaneHeaderFrame pane={null} multi name="a" actions={null} />)
  expect(window.hasAttribute("data-tauri-drag-region")).toBe(true)
  expect(window.hasAttribute("data-split-keeps")).toBe(false)
})

it("keeps a hidden name's place and says nothing of it", () => {
  const hidden = header(
    <PaneHeaderFrame pane={1} multi={false} name="a" nameShown={false} actions={null} />,
  )
  const name = hidden.querySelector(".workspace-pane-name") as HTMLElement
  expect(name.getAttribute("aria-hidden")).toBe("true")
  expect(name.hasAttribute("data-shown")).toBe(false)
  const shown = header(<PaneHeaderFrame pane={1} multi={false} name="a" actions={null} />)
  expect(shown.querySelector(".workspace-pane-name")?.hasAttribute("data-shown")).toBe(
    true,
  )
})
