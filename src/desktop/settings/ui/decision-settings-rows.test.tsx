// @vitest-environment jsdom
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import type { DecisionSettings } from "@nessa/client"
import type { DecisionSettingsGateway } from "../adapters/decision-settings-gateway"
import { DecisionSettingsProvider, DecisionSettingsRows } from "./decision-settings-rows"

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

/** A gateway that keeps what it is given, as the real one does. */
function keeping(
  start: DecisionSettings = { autoPick: true, autoPickAfterSeconds: 300 },
) {
  let kept = start
  const gateway = {
    read: vi.fn(async () => kept),
    set: vi.fn(async (next: DecisionSettings) => {
      kept = next
      return kept
    }),
  } satisfies DecisionSettingsGateway
  return gateway
}

const settle = () => act(async () => {})

async function draw(gateway: DecisionSettingsGateway | undefined) {
  act(() =>
    root.render(
      <DecisionSettingsProvider gateway={gateway}>
        <DecisionSettingsRows />
      </DecisionSettingsProvider>,
    ),
  )
  await settle()
}

const toggle = () => container.querySelector<HTMLButtonElement>('[role="switch"]')
const amount = () =>
  container.querySelector<HTMLInputElement>("[data-auto-pick-amount]") as HTMLInputElement
const typeAndEnter = async (text: string) => {
  const input = amount()
  act(() => {
    const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set
    set?.call(input, text)
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
  act(() => {
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }))
  })
  await settle()
}

it("shows the kept wait in its largest whole unit", async () => {
  await draw(keeping({ autoPick: true, autoPickAfterSeconds: 7200 }))
  expect(amount().value).toBe("2")
  expect(container.textContent).toContain("hours")
  expect(toggle()?.getAttribute("aria-checked")).toBe("true")
})

it("turns auto-pick off, and the wait rests with it", async () => {
  const gateway = keeping()
  await draw(gateway)
  act(() => toggle()?.click())
  await settle()
  expect(gateway.set).toHaveBeenCalledWith({ autoPick: false, autoPickAfterSeconds: 300 })
  expect(toggle()?.getAttribute("aria-checked")).toBe("false")
  expect(amount().disabled).toBe(true)
})

it("saves a typed wait, and says what it takes when it is out of range", async () => {
  const gateway = keeping()
  await draw(gateway)
  await typeAndEnter("12")
  expect(gateway.set).toHaveBeenLastCalledWith({
    autoPick: true,
    autoPickAfterSeconds: 720,
  })
  await typeAndEnter("0")
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(
    "1 to 1,440 minutes.",
  )
  await typeAndEnter("1441")
  expect(gateway.set).toHaveBeenCalledTimes(1)
})

it("says it could not read, and reads again on Reload", async () => {
  const gateway = keeping()
  gateway.read.mockRejectedValueOnce(new Error("offline"))
  await draw(gateway)
  expect(container.textContent).toContain("Couldn’t read this.")
  const reload = [...container.querySelectorAll("button")].find(
    (button) => button.textContent === "Reload",
  )
  act(() => reload?.click())
  await settle()
  expect(container.textContent).not.toContain("Couldn’t read this.")
  expect(amount().value).toBe("5")
})

it("rests with no gateway", async () => {
  await draw(undefined)
  expect(container.textContent).toContain("Connect a gateway")
  expect(toggle()?.disabled).toBe(true)
})
