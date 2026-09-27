// @vitest-environment jsdom
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { ComposerTray, type TrayApproval } from "./composer-tray"

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  container = document.createElement("div")
  document.body.append(container)
  root = createRoot(container)
})

afterEach(() => {
  act(() => root.unmount())
  container.remove()
})

function render(props: Partial<React.ComponentProps<typeof ComposerTray>> = {}) {
  const onChoose = vi.fn()
  act(() => {
    root.render(<ComposerTray disabled={false} onChoose={onChoose} {...props} />)
  })
  return { onChoose }
}

function plus() {
  const button = container.querySelector<HTMLButtonElement>("button[aria-expanded]")
  if (!button) throw new Error("no + button")
  return button
}

function row(name: string) {
  const found = [
    ...container.querySelectorAll<HTMLButtonElement>("[role=dialog] button"),
  ].find((button) => button.textContent?.startsWith(name))
  if (!found) throw new Error(`no row ${name}`)
  return found
}

function click(element: HTMLElement) {
  act(() => element.click())
}

function key(name: string) {
  act(() => {
    document.dispatchEvent(new KeyboardEvent("keydown", { key: name, bubbles: true }))
  })
}

function tray() {
  return container.querySelector("[role=dialog]")
}

function approval(overrides: Partial<TrayApproval> = {}): TrayApproval {
  return { mode: "ask", modes: ["ask", "auto", "full"], onChange: vi.fn(), ...overrides }
}

it("opens a tray above the composer and closes it again from the same button", () => {
  render()
  expect(tray()).toBeNull()

  click(plus())
  expect(tray()).not.toBeNull()
  expect(plus().getAttribute("aria-expanded")).toBe("true")
  expect(document.activeElement).toBe(row("Add files"))

  click(plus())
  expect(tray()).toBeNull()
})

it("chooses files and closes", () => {
  const { onChoose } = render()
  click(plus())
  click(row("Add files"))

  expect(onChoose).toHaveBeenCalledOnce()
  expect(tray()).toBeNull()
})

it("keeps Add files unavailable while earlier files are still being read", () => {
  const { onChoose } = render({ disabled: true, onSignOut: vi.fn() })
  click(plus())

  expect(row("Add files").disabled).toBe(true)
  expect(onChoose).not.toHaveBeenCalled()
})

it("is only a files button, disabled with them, when there is nothing else to offer", () => {
  render({ disabled: true })
  expect(plus().disabled).toBe(true)
})

it("offers sign out when the surface has one", () => {
  const onSignOut = vi.fn()
  render({ onSignOut })
  click(plus())
  click(row("Sign out"))

  expect(onSignOut).toHaveBeenCalledOnce()
  expect(tray()).toBeNull()
})

it("shows no approval row until the gateway reports a mode", () => {
  render()
  click(plus())
  expect(tray()?.textContent).not.toContain("Tool approval")
})

it("lets the approval row take the tray over and changes the mode", () => {
  const choice = approval()
  render({ approval: choice })
  click(plus())
  expect(row("Tool approval").textContent).toBe("Tool approvalAsk first")

  click(row("Tool approval"))
  expect(tray()?.textContent).not.toContain("Add files")
  const radios = container.querySelectorAll("[role=radio]")
  expect([...radios].map((radio) => radio.getAttribute("aria-checked"))).toEqual([
    "true",
    "false",
    "false",
  ])

  click(row("Automatic"))
  expect(choice.onChange).toHaveBeenCalledExactlyOnceWith("auto")
  expect(tray()?.textContent).toContain("Add files")
})

it("does not ask to change to the mode already in force", () => {
  const choice = approval()
  render({ approval: choice })
  click(plus())
  click(row("Tool approval"))
  click(row("Ask first"))

  expect(choice.onChange).not.toHaveBeenCalled()
})

it("offers only what the agent honours, and says why the rest are missing", () => {
  const choice = approval({ modes: ["ask"] })
  render({ approval: choice })
  click(plus())
  click(row("Tool approval"))

  expect(row("Full access").disabled).toBe(true)
  expect(row("Full access").textContent).toContain("Not available for this agent.")
  expect(row("Ask first").disabled).toBe(false)
})

it("steps back one page on Escape, then closes and returns focus to +", () => {
  render({ approval: approval() })
  click(plus())
  click(row("Tool approval"))

  key("Escape")
  expect(tray()?.textContent).toContain("Add files")

  key("Escape")
  expect(tray()).toBeNull()
  expect(document.activeElement).toBe(plus())
})

it("closes on a press outside it, but not on one inside", () => {
  render({ approval: approval() })
  click(plus())

  act(() => {
    row("Tool approval").dispatchEvent(new Event("pointerdown", { bubbles: true }))
  })
  expect(tray()).not.toBeNull()

  act(() => {
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }))
  })
  expect(tray()).toBeNull()
})

function rect(top: number, left: number, width: number, height: number) {
  return DOMRect.fromRect({ x: left, y: top, width, height })
}

/** The tray inside a composer and its row, as the panel renders it. */
function renderInComposer() {
  act(() => {
    root.render(
      <div data-slot="pill-composer">
        <div data-slot="pill-composer-row">
          <ComposerTray disabled={false} onChoose={() => {}} />
        </div>
      </div>,
    )
  })
  const composer = container.querySelector<HTMLElement>('[data-slot="pill-composer"]')
  const row = container.querySelector<HTMLElement>('[data-slot="pill-composer-row"]')
  const wrapper = plus().parentElement
  if (!composer || !row || !wrapper) throw new Error("composer did not render")
  // The + sits in the row's bottom-left corner; the composer holds an
  // attachment strip above the row.
  wrapper.getBoundingClientRect = () => rect(560, 26, 36, 36)
  row.getBoundingClientRect = () => rect(550, 20, 380, 56)
  composer.getBoundingClientRect = () => rect(480, 20, 380, 126)
  return { composer }
}

it("rises clear of the whole composer, attachments included, when it has a box", () => {
  const { composer } = renderInComposer()
  composer.getClientRects = () => [rect(480, 20, 380, 126)] as unknown as DOMRectList
  click(plus())

  const style = (tray() as HTMLElement).style
  expect(style.bottom).toBe(`${596 - 480 + 10}px`)
  expect(style.left).toBe("-6px")
  expect(style.width).toBe("336px")
})

it("measures the row when the composer is unwrapped on a layout compositor", () => {
  const { composer } = renderInComposer()
  composer.getClientRects = () => [] as unknown as DOMRectList
  click(plus())

  const style = (tray() as HTMLElement).style
  expect(style.bottom).toBe(`${596 - 550 + 10}px`)
  expect(style.left).toBe("-6px")
})
