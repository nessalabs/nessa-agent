import * as React from "react"
import { renderToStaticMarkup } from "react-dom/server"
import { describe, expect, it, vi } from "vitest"

import { HandoffFailed } from "./setup-gate"
import { setupRecovery } from "../application/setup-recovery"
import type { SetupHandoff } from "../../host"

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

/** The screen a given handoff leaves, decided the way the surface decides it. */
function screenFor(handoff: SetupHandoff, closeFailed = false) {
  const recovery = setupRecovery(handoff)
  expect(recovery, "this handoff leaves no screen").not.toBeNull()
  return { recovery: recovery!, closeFailed }
}

const panelUnavailable = screenFor({
  outcome: "panel-unavailable",
  cause: new Error("there is no panel to summon"),
})
const closeRefused = screenFor({
  outcome: "setup-close-failed",
  panelShown: true,
  cause: "could not close setup: the window server said no",
})
const notRecorded = screenFor({
  outcome: "setup-not-recorded",
  cause: "could not record that setup finished: disk full",
})

describe("the screen left when the panel did not come up", () => {
  it("offers both ways out as real buttons", () => {
    const markup = renderToStaticMarkup(
      React.createElement(HandoffFailed, {
        ...panelUnavailable,
        onRetry: () => {},
        onSaveAgain: () => {},
        onClose: () => {},
      }),
    )
    // Keyboard-reachable by being buttons, not by anything this screen adds.
    expect(markup).toContain('<button type="button"')
    expect(markup).toContain("Try again")
    expect(markup).toContain("Close this window")
    expect(markup).toContain("Nessa could not open the panel")
    expect(markup).toContain('role="alertdialog"')
    // The alert is where focus is put, so it has to be able to take it.
    expect(markup).toContain('tabindex="-1"')
    // Nothing claims the close failed until one has.
    expect(markup).not.toContain("would not close")
  })

  it("asks for the handoff again, and for the window to close", () => {
    const onRetry = vi.fn()
    const onClose = vi.fn()
    const screen = HandoffFailed({
      ...panelUnavailable,
      onRetry,
      onSaveAgain: () => {},
      onClose,
    })

    press(screen, "Try again")
    expect(onRetry).toHaveBeenCalledTimes(1)
    expect(onClose).not.toHaveBeenCalled()

    press(screen, "Close this window")
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it("keeps its buttons when the close itself failed, and says so", () => {
    const markup = renderToStaticMarkup(
      React.createElement(HandoffFailed, {
        ...screenFor(
          { outcome: "panel-unavailable", cause: new Error("no panel") },
          true,
        ),
        onRetry: () => {},
        onSaveAgain: () => {},
        onClose: () => {},
      }),
    )
    expect(markup).toContain("Try again")
    expect(markup).toContain("Close this window")
    expect(markup).toContain("This window still would not close")
    expect(markup).toContain('role="status"')
    // This window is undecorated: it has no window controls to be sent to.
    expect(markup).not.toContain("window controls")
  })
})

describe("the screen left when the panel came up and this window did not go", () => {
  it("says the panel is ready rather than blaming the summon", () => {
    const markup = renderToStaticMarkup(
      React.createElement(HandoffFailed, {
        ...closeRefused,
        onRetry: () => {},
        onSaveAgain: () => {},
        onClose: () => {},
      }),
    )
    expect(markup).toContain("Nessa is open behind this window")
    expect(markup).toContain("the panel is ready")
    expect(markup).not.toContain("Nessa could not open the panel")
  })

  it("offers the close alone, because there is nothing to hand over again", () => {
    const onRetry = vi.fn()
    const onClose = vi.fn()
    const screen = HandoffFailed({
      ...closeRefused,
      onRetry,
      onSaveAgain: () => {},
      onClose,
    })

    // Retrying would summon a panel already standing behind this window, and
    // re-run a completion already written.
    expect(buttons(screen).map(label)).toEqual(["Close this window"])
    press(screen, "Close this window")
    expect(onClose).toHaveBeenCalledTimes(1)
    expect(onRetry).not.toHaveBeenCalled()
  })
})

describe("the screen left when the panel came up and nothing was written down", () => {
  it("says the panel works and that the choice was not kept", () => {
    const markup = renderToStaticMarkup(
      React.createElement(HandoffFailed, {
        ...notRecorded,
        onRetry: () => {},
        onSaveAgain: () => {},
        onClose: () => {},
      }),
    )
    expect(markup).toContain("Nessa is open, but setup was not saved")
    // Both halves of the cost, because closing this window loses the choice
    // without either of them having been said.
    expect(markup).toContain("ask again next time")
    expect(markup).toContain("default agent")
    // Not an outage: the panel is up and usable.
    expect(markup).not.toContain("Nessa could not open the panel")
  })

  it("offers the write again and not the whole handoff", () => {
    const onRetry = vi.fn()
    const onSaveAgain = vi.fn()
    const onClose = vi.fn()
    const screen = HandoffFailed({ ...notRecorded, onRetry, onSaveAgain, onClose })

    // Handing over again would summon a panel already on screen, re-anchoring
    // and refitting a window somebody may have moved to.
    expect(buttons(screen).map(label)).toEqual(["Save again", "Close this window"])
    press(screen, "Save again")
    expect(onSaveAgain).toHaveBeenCalledTimes(1)
    expect(onRetry).not.toHaveBeenCalled()
  })
})
