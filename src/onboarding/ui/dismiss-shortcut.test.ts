import { describe, expect, it, vi } from "vitest"
import {
  listenForDismiss,
  type DismissKeyPress,
  type DismissKeyTarget,
} from "./dismiss-shortcut"

/** A stand-in for `window` that records what is listening to it. */
function keyTarget() {
  const handlers = new Set<(event: DismissKeyPress) => void>()
  const target: DismissKeyTarget = {
    addEventListener(_type, handler) {
      handlers.add(handler)
    },
    removeEventListener(_type, handler) {
      handlers.delete(handler)
    },
  }
  return {
    target,
    get listening() {
      return handlers.size
    },
    press(press: Partial<DismissKeyPress> & { key: string }) {
      const event: DismissKeyPress = {
        repeat: false,
        metaKey: false,
        ctrlKey: false,
        preventDefault: vi.fn(),
        ...press,
      }
      for (const handler of handlers) handler(event)
      return event
    },
  }
}

describe("the keys that leave setup", () => {
  it.each([{ key: "Escape" }, { key: "q", metaKey: true }, { key: "Q", ctrlKey: true }])(
    "takes %j while setup is showing",
    (press) => {
      const keys = keyTarget()
      const dismiss = vi.fn()
      listenForDismiss(keys.target, true, dismiss)
      const event = keys.press(press)
      expect(event.preventDefault).toHaveBeenCalled()
      expect(dismiss).toHaveBeenCalledTimes(1)
    },
  )

  it("leaves other keys alone", () => {
    const keys = keyTarget()
    const dismiss = vi.fn()
    listenForDismiss(keys.target, true, dismiss)
    for (const press of [
      { key: "q" },
      { key: "Escape", repeat: true },
      { key: "Enter", metaKey: true },
    ]) {
      expect(keys.press(press).preventDefault).not.toHaveBeenCalled()
    }
    expect(dismiss).not.toHaveBeenCalled()
  })

  // The browser path keeps the setup component mounted after setup finishes: it
  // starts rendering the panel instead. A listener left attached went on eating
  // every Escape and Command-Q from then on, and neither the panel nor the
  // browser saw them again.
  it("listens to nothing once setup is no longer showing", () => {
    const keys = keyTarget()
    const dismiss = vi.fn()
    const stop = listenForDismiss(keys.target, false, dismiss)
    expect(keys.listening).toBe(0)
    const event = keys.press({ key: "Escape" })
    expect(event.preventDefault).not.toHaveBeenCalled()
    expect(dismiss).not.toHaveBeenCalled()
    stop()
    expect(keys.listening).toBe(0)
  })

  it("stops listening when setup is left behind", () => {
    const keys = keyTarget()
    const dismiss = vi.fn()
    const stop = listenForDismiss(keys.target, true, dismiss)
    expect(keys.listening).toBe(1)
    stop()
    expect(keys.listening).toBe(0)
    expect(keys.press({ key: "Escape" }).preventDefault).not.toHaveBeenCalled()
    expect(dismiss).not.toHaveBeenCalled()
  })
})
