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
const field = () => {
  const input = host.querySelector("input")
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
    host
      .querySelector("form")
      ?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }))
  })
}

it("clears a saved token and ignores an older status read, then disconnects", async () => {
  let finishRead!: (value: BoatStatus) => void
  const calls: string[] = []
  const gateway: BoatGateway = {
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
  expect(field().type).toBe("password")
  await enter("  fixture-token  ")
  await submit()
  expect(calls).toEqual(["fixture-token"])
  expect(field().value).toBe("")
  await act(async () => finishRead({ state: "disconnected" }))
  const disconnect = [...host.querySelectorAll("button")].find(
    (button) => button.textContent === "Disconnect",
  )
  expect(disconnect).toBeDefined()
  await act(async () => disconnect?.click())
  expect(calls).toEqual(["fixture-token", "disconnect"])
  expect(host.textContent).not.toContain("Replace token")
})

it("reports a rejected save without claiming a connection", async () => {
  const gateway: BoatGateway = {
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
  await enter("fixture-token")
  await submit()
  expect(host.querySelector('[role="alert"]')?.textContent).toContain(
    "could not be connected",
  )
  expect(
    [...host.querySelectorAll("button")].some(
      (button) => button.textContent === "Disconnect",
    ),
  ).toBe(false)
  expect(host.querySelector('button[type="submit"]')?.hasAttribute("disabled")).toBe(
    false,
  )
})

it("rests without a connected gateway", async () => {
  await act(async () => root.render(<BoatRow />))
  expect(field().disabled).toBe(true)
  expect(host.querySelector('button[type="submit"]')?.hasAttribute("disabled")).toBe(true)
})
