// @vitest-environment jsdom
import * as React from "react"
import { createRoot } from "react-dom/client"
import { expect, it, vi } from "vitest"
import { useKeyBindings } from "./shortcuts"

vi.mock("../../../adapters/platform", () => ({ isMac: true }))

const bindings = [{ chord: { code: "KeyN", command: true }, command: "new" }] as const

async function mounted(run: (command: "new", event: KeyboardEvent) => void) {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  function Bound() {
    useKeyBindings(bindings, run)
    return null
  }
  const host = document.createElement("div")
  document.body.append(host)
  const root = createRoot(host)
  await React.act(async () => root.render(React.createElement(Bound)))
  return async () => {
    await React.act(async () => root.unmount())
    host.remove()
  }
}

it.each([true, false])(
  "consumes an owned chord when the action is available: %s",
  async (available) => {
    const applied: string[] = []
    const run = vi.fn((command: "new", event: KeyboardEvent) => {
      expect(event.defaultPrevented).toBe(true)
      if (!available) return false
      applied.push(command)
    })
    const close = await mounted(run)
    try {
      const event = new KeyboardEvent("keydown", {
        code: "KeyN",
        metaKey: true,
        cancelable: true,
      })
      window.dispatchEvent(event)
      expect(event.defaultPrevented).toBe(true)
      expect(run).toHaveBeenCalledTimes(1)
      expect(applied).toEqual(available ? ["new"] : [])
    } finally {
      await close()
    }
  },
)

it("leaves an unbound key to its existing owner", async () => {
  const run = vi.fn()
  const close = await mounted(run)
  try {
    const event = new KeyboardEvent("keydown", {
      code: "KeyX",
      metaKey: true,
      cancelable: true,
    })
    window.dispatchEvent(event)
    expect(event.defaultPrevented).toBe(false)
    expect(run).not.toHaveBeenCalled()
  } finally {
    await close()
  }
})

it("retains key ownership when its command throws, and reports the original error", async () => {
  const failure = new Error("command failed")
  const observed: unknown[] = []
  const onError = (event: ErrorEvent) => {
    observed.push(event.error)
    event.preventDefault()
  }
  window.addEventListener("error", onError)
  const close = await mounted(() => {
    throw failure
  })
  try {
    const event = new KeyboardEvent("keydown", {
      code: "KeyN",
      metaKey: true,
      cancelable: true,
    })
    window.dispatchEvent(event)
    expect(event.defaultPrevented).toBe(true)
    expect(observed).toEqual([failure])
  } finally {
    window.removeEventListener("error", onError)
    await close()
  }
})
