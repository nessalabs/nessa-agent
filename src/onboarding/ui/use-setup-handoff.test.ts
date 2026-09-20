// @vitest-environment jsdom
/**
 * What the end of setup does to its window, run the way React runs it.
 *
 * The decisions are tested as plain functions in `setup-handoff.test.ts`. This
 * is the other half: the effects that perform them, under Strict Mode because
 * `main.tsx` turns it on for this window, against a host that answers when the
 * test says so. The one property worth the DOM is that `finishSetupWindow` —
 * which shows the panel, writes setup off for good and closes this window — is
 * called exactly once per attempt however many times React runs the effect.
 */
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { SetupHandoff, SetupWindowClose } from "../../host"

const host = vi.hoisted(() => ({
  revealSetupWindow: vi.fn<() => Promise<void>>(),
  finishSetupWindow: vi.fn<(completed: boolean) => Promise<SetupHandoff>>(),
  closeSetupWindow: vi.fn<() => Promise<SetupWindowClose>>(),
}))
vi.mock("../../host", () => host)

import { HandoffFailed } from "./setup-gate"
import { useSetupHandoff } from "./use-setup-handoff"

/** A promise the test settles by hand, so the host answers when told to. */
function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (cause: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

/** The branch of `SetupGate` this hook drives, and nothing else of it. */
function Surface({
  setupActive,
  completed,
}: {
  setupActive: boolean
  completed: boolean
}) {
  const handoff = useSetupHandoff(setupActive, completed)
  if (!handoff.recovery) return null
  return React.createElement(HandoffFailed, {
    ref: handoff.dialog,
    recovery: handoff.recovery,
    onRetry: handoff.retry,
    onClose: handoff.close,
    closeFailed: handoff.closeFailed,
  })
}

let container: HTMLDivElement
let root: Root

/** Let React commit and every pending promise run. */
async function flush() {
  await React.act(async () => {
    await Promise.resolve()
  })
}

async function render(props: { setupActive: boolean; completed: boolean }) {
  await React.act(async () => {
    root.render(
      React.createElement(React.StrictMode, null, React.createElement(Surface, props)),
    )
  })
}

function button(name: string): HTMLButtonElement {
  const found = Array.from(container.querySelectorAll("button")).find(
    (candidate) => candidate.textContent === name,
  )
  expect(found, `no ${name} button`).toBeDefined()
  return found!
}

async function press(name: string) {
  const control = button(name)
  await React.act(async () => {
    control.focus()
    control.click()
  })
}

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host.revealSetupWindow.mockReset().mockResolvedValue(undefined)
  host.finishSetupWindow.mockReset()
  host.closeSetupWindow.mockReset()
  container = document.createElement("div")
  document.body.appendChild(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

describe("revealing the window", () => {
  it("asks the host to show it as soon as it has rendered", async () => {
    host.finishSetupWindow.mockReturnValue(deferred<SetupHandoff>().promise)
    await render({ setupActive: true, completed: false })
    expect(host.revealSetupWindow).toHaveBeenCalled()
    // Setup is still on screen, so nothing is handed over yet.
    expect(host.finishSetupWindow).not.toHaveBeenCalled()
  })
})

describe("handing over once setup is over", () => {
  it("calls the host exactly once per attempt, under Strict Mode and re-renders", async () => {
    const answer = deferred<SetupHandoff>()
    host.finishSetupWindow.mockReturnValue(answer.promise)

    await render({ setupActive: false, completed: true })
    // Strict Mode has already run the effect twice by now.
    expect(host.finishSetupWindow).toHaveBeenCalledTimes(1)

    await render({ setupActive: false, completed: true })
    await render({ setupActive: false, completed: true })
    expect(host.finishSetupWindow).toHaveBeenCalledTimes(1)
    // Nothing is on screen while the answer is outstanding.
    expect(container.querySelector('[role="alertdialog"]')).toBeNull()
  })

  it.each([true, false])("carries how setup ended (completed=%s)", async (completed) => {
    host.finishSetupWindow.mockReturnValue(deferred<SetupHandoff>().promise)
    await render({ setupActive: false, completed })
    expect(host.finishSetupWindow).toHaveBeenCalledWith(completed)
  })

  it("leaves nothing on screen when the panel came up and this window is going", async () => {
    host.finishSetupWindow.mockResolvedValue({ outcome: "handed-over" })
    await render({ setupActive: false, completed: true })
    await flush()
    expect(container.innerHTML).toBe("")
    expect(host.finishSetupWindow).toHaveBeenCalledTimes(1)
  })

  it("survives the window going before the host answers", async () => {
    const answer = deferred<SetupHandoff>()
    host.finishSetupWindow.mockReturnValue(answer.promise)
    await render({ setupActive: false, completed: true })

    await React.act(async () => root.unmount())
    answer.resolve({ outcome: "panel-unavailable", cause: new Error("late") })
    await flush()

    expect(host.finishSetupWindow).toHaveBeenCalledTimes(1)
    expect(container.innerHTML).toBe("")
    // `afterEach` unmounts again; a second unmount of an empty root is a no-op.
  })
})

describe("the screen a failed handoff leaves", () => {
  async function failOnce() {
    const answer = deferred<SetupHandoff>()
    host.finishSetupWindow.mockReturnValueOnce(answer.promise)
    await render({ setupActive: false, completed: true })
    await React.act(async () => {
      answer.resolve({ outcome: "panel-unavailable", cause: new Error("no panel") })
    })
    await flush()
  }

  it("puts the caret on the alert, because setup's own controls have gone", async () => {
    await failOnce()
    const dialog = container.querySelector('[role="alertdialog"]')
    expect(dialog).not.toBeNull()
    expect(document.activeElement).toBe(dialog)
  })

  it("is shown for a host call that rejected, too", async () => {
    host.finishSetupWindow.mockRejectedValueOnce(new Error("invoke failed"))
    await render({ setupActive: false, completed: true })
    await flush()
    expect(container.querySelector('[role="alertdialog"]')).not.toBeNull()
    expect(container.textContent).toContain("Try again")
  })

  it("makes exactly one more host call on retry", async () => {
    await failOnce()
    const again = deferred<SetupHandoff>()
    host.finishSetupWindow.mockReturnValueOnce(again.promise)

    await press("Try again")
    expect(host.finishSetupWindow).toHaveBeenCalledTimes(2)
    expect(host.finishSetupWindow).toHaveBeenLastCalledWith(true)
    // The screen is gone while the new attempt is outstanding.
    expect(container.querySelector('[role="alertdialog"]')).toBeNull()

    await render({ setupActive: false, completed: true })
    expect(host.finishSetupWindow).toHaveBeenCalledTimes(2)
  })

  it("keeps the screen, and the focus, when the close itself fails", async () => {
    await failOnce()
    host.closeSetupWindow.mockResolvedValue({
      outcome: "close-failed",
      cause: "the window server said no",
    })

    await press("Close this window")
    await flush()

    expect(host.closeSetupWindow).toHaveBeenCalledTimes(1)
    expect(container.querySelector('[role="alertdialog"]')).not.toBeNull()
    expect(container.querySelector('[role="status"]')?.textContent).toContain(
      "still would not close",
    )
    expect(document.activeElement).toBe(button("Close this window"))
    // A failed close is not a reason to hand over again.
    expect(host.finishSetupWindow).toHaveBeenCalledTimes(1)
  })
})
