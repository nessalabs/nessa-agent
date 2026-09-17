import * as React from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it, vi } from "vitest"

import { HandoffFailed } from "./setup-gate"

/** Every element in a rendered tree, so a control can be found and pressed. */
function elements(node: React.ReactNode): React.ReactElement[] {
  if (Array.isArray(node)) return node.flatMap(elements)
  if (!React.isValidElement(node)) return []
  const { children } = node.props as { children?: React.ReactNode }
  return [node, ...elements(children)]
}

function buttons(screen: React.ReactNode) {
  return elements(screen).filter((element) => element.type === "button")
}

function label(button: React.ReactElement) {
  return String((button.props as { children?: React.ReactNode }).children)
}

function press(screen: React.ReactNode, name: string) {
  const button = buttons(screen).find((candidate) => label(candidate) === name)
  expect(button, `no ${name} button`).toBeDefined()
  ;(button?.props as { onClick?: () => void }).onClick?.()
}

describe("the screen left when the panel did not come up", () => {
  it("offers both ways out as real buttons", () => {
    const markup = renderToStaticMarkup(
      React.createElement(HandoffFailed, {
        onRetry: () => {},
        onClose: () => {},
        closeFailed: false,
      }),
    )
    // Keyboard-reachable by being buttons, not by anything this screen adds.
    expect(markup).toContain('<button type="button"')
    expect(markup).toContain("Try again")
    expect(markup).toContain("Close this window")
    expect(markup).toContain('role="alertdialog"')
    // Nothing claims the close failed until one has.
    expect(markup).not.toContain("would not close")
  })

  it("asks for the handoff again, and for the window to close", () => {
    const onRetry = vi.fn()
    const onClose = vi.fn()
    const screen = HandoffFailed({ onRetry, onClose, closeFailed: false })

    press(screen, "Try again")
    expect(onRetry).toHaveBeenCalledTimes(1)
    expect(onClose).not.toHaveBeenCalled()

    press(screen, "Close this window")
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it("keeps both buttons when the close itself failed, and says so", () => {
    const markup = renderToStaticMarkup(
      React.createElement(HandoffFailed, {
        onRetry: () => {},
        onClose: () => {},
        closeFailed: true,
      }),
    )
    expect(markup).toContain("Try again")
    expect(markup).toContain("Close this window")
    expect(markup).toContain("This window would not close")
    expect(markup).toContain('role="status"')
  })
})
