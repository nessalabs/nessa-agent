// @vitest-environment jsdom
import { act } from "react"
import { createRoot } from "react-dom/client"
import { expect, it } from "vitest"
import { IconButton } from "./icon-button"

async function render(node: React.ReactNode) {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  await act(async () => root.render(node))
  return {
    button: host.querySelector("button") as HTMLButtonElement,
    done: async () => {
      await act(async () => root.unmount())
      host.remove()
    },
  }
}

it("names itself and its tooltip one way: the label, then its shortcut", async () => {
  const { button, done } = await render(
    <IconButton icon="close" label="Close Pane" shortcut="⌘W" />,
  )
  expect(button.getAttribute("aria-label")).toBe("Close Pane (⌘W)")
  expect(button.dataset.tooltip).toBe("Close Pane")
  expect(button.dataset.tooltipShortcut).toBe("⌘W")
  await done()
})

it("is a plain label without a shortcut, at the window's control size, rounded and muted", async () => {
  const { button, done } = await render(<IconButton icon="close" label="Close" />)
  expect(button.getAttribute("aria-label")).toBe("Close")
  expect(button.dataset.tooltipShortcut).toBeUndefined()
  expect([button.dataset.size, button.dataset.shape, button.dataset.tone]).toEqual([
    "md",
    "rounded",
    "muted",
  ])
  await done()
})

it("says its size, shape and tone, and the side its tooltip prefers", async () => {
  const { button, done } = await render(
    <IconButton
      icon="appearance"
      label="Appearance"
      size="sm"
      shape="pill"
      tone="faint"
      tooltipSide="above"
    />,
  )
  expect([button.dataset.size, button.dataset.shape, button.dataset.tone]).toEqual([
    "sm",
    "pill",
    "faint",
  ])
  expect(button.dataset.tooltipSide).toBe("above")
  await done()
})

it("lends its look to another button-shaped component", async () => {
  const { button, done } = await render(
    <IconButton
      as={(props) => <button {...props} data-borrowed="" />}
      icon="sidebar"
      label="Hide Sidebar"
      className="extra"
    />,
  )
  expect(button.dataset.borrowed).toBe("")
  expect(button.className).toBe("desktop-icon-button extra")
  expect(button.getAttribute("aria-label")).toBe("Hide Sidebar")
  await done()
})
