// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { BoatStatus } from "@nessa/client"
import type { BoatGateway } from "../adapters/boat-gateway"
import { BoatProvider, BoatRow } from "./boat-row"

let root: Root
let host: HTMLDivElement
beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  host = document.createElement("div")
  document.body.append(host)
  root = createRoot(host)
})
afterEach(async () => {
  await act(async () => root.unmount())
  host.remove()
})
const connected: BoatStatus = {
  state: "connected",
  environment: "nessa",
  image: "fixture-image",
}
/** The row's own button, by name. */
const button = (name: string) =>
  [...document.querySelectorAll("button")].find((each) => each.textContent === name)
/** Connect… opens the dialog the token is pasted in. */
async function ask() {
  await act(async () => button("Connect…")?.click())
}
const field = () => {
  const input = document.querySelector<HTMLInputElement>("[data-boat-dialog] input")
  if (!input) throw new Error("missing token field")
  return input
}
async function enter(token: string) {
  await act(async () => {
    const setter = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "value",
    )?.set
    if (!setter) throw new Error("missing input setter")
    setter.call(field(), token)
    field().dispatchEvent(new Event("input", { bubbles: true }))
  })
}
async function submit() {
  await act(async () => {
    document
      .querySelector("[data-boat-dialog] form")
      ?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }))
  })
}

it("clears a saved token and ignores an older status read, then disconnects", async () => {
  let finishRead!: (value: BoatStatus) => void
  const calls: string[] = []
  const gateway: BoatGateway = {
    runtimeSettings: async () => ({ defaultLifetime: { mode: "never" } }),
    setRuntimeSettings: async () => ({ defaultLifetime: { mode: "never" } }),
    read: () =>
      new Promise((resolve) => {
        finishRead = resolve
      }),
    save: async (token) => {
      calls.push(token)
      return connected
    },
    disconnect: async () => {
      calls.push("disconnect")
      return { state: "disconnected" }
    },
  }
  await act(async () =>
    root.render(
      <BoatProvider gateway={gateway}>
        <BoatRow />
      </BoatProvider>,
    ),
  )
  // One word of state and one button; the field is only in the dialog.
  expect(host.querySelector("input")).toBeNull()
  await ask()
  expect(field().type).toBe("password")
  await enter("  fixture-token  ")
  await submit()
  expect(calls).toEqual(["fixture-token"])
  // Saved, the dialog closes and the row says so.
  expect(document.querySelector("[data-boat-dialog]")).toBeNull()
  expect(host.querySelector('[data-setting="boat"]')?.textContent).toContain("Connected")
  await act(async () => finishRead({ state: "disconnected" }))
  const disconnect = button("Disconnect")
  expect(disconnect).toBeDefined()
  await act(async () => disconnect?.click())
  expect(calls).toEqual(["fixture-token", "disconnect"])
  expect(host.textContent).not.toContain("Replace token")
})

it("reports a rejected save without claiming a connection", async () => {
  const gateway: BoatGateway = {
    runtimeSettings: async () => ({ defaultLifetime: { mode: "never" } }),
    setRuntimeSettings: async () => ({ defaultLifetime: { mode: "never" } }),
    read: async () => ({ state: "disconnected" }),
    save: async () => {
      throw new Error("fixture provider unavailable")
    },
    disconnect: async () => ({ state: "disconnected" }),
  }
  await act(async () =>
    root.render(
      <BoatProvider gateway={gateway}>
        <BoatRow />
      </BoatProvider>,
    ),
  )
  expect(host.querySelector('[data-setting="boat"]')?.textContent).toContain(
    "Not connected",
  )
  await ask()
  await enter("fixture-token")
  await submit()
  // The dialog stays open and says why.
  expect(
    document.querySelector('[data-boat-dialog] [role="alert"]')?.textContent,
  ).toContain("could not be connected")
  expect(button("Disconnect")).toBeUndefined()
  expect(
    document
      .querySelector('[data-boat-dialog] button[type="submit"]')
      ?.hasAttribute("disabled"),
  ).toBe(false)
})

it("rests without a connected gateway", async () => {
  await act(async () => root.render(<BoatRow />))
  expect(button("Connect…")?.hasAttribute("disabled")).toBe(true)
})
