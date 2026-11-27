// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import { NessaBoatError, type BoatStatus } from "@nessa/client"
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
  vi.useRealTimers()
})
const URL_ = "https://boat.dev/api/boat/oauth2/authorize?client_id=boatapp_x"
const waiting: BoatStatus = {
  state: "disconnected",
  signInAvailable: true,
  signIn: { state: "waiting", url: URL_ },
}
const signedIn: BoatStatus = {
  state: "connected",
  environment: "nessa",
  image: "fixture-image",
  via: "signIn",
  login: "octo",
  signInAvailable: true,
}
/** The row's own button, by name. */
const button = (name: string) =>
  [...document.querySelectorAll("button")].find((each) => each.textContent === name)
const row = () => host.querySelector('[data-setting="boat"]')?.textContent ?? ""

function fixture(reads: BoatStatus[]) {
  const calls: string[] = []
  let status: BoatStatus = { state: "disconnected", signInAvailable: true }
  const gateway: BoatGateway = {
    read: async () => {
      calls.push("read")
      status = reads.shift() ?? status
      return status
    },
    signIn: async () => {
      calls.push("signIn")
      status = waiting
      return status
    },
    cancelSignIn: async () => {
      calls.push("cancel")
      status = { state: "disconnected", signInAvailable: true }
      return status
    },
    disconnect: async () => {
      calls.push("disconnect")
      status = { state: "disconnected", signInAvailable: true }
      return status
    },
    open: (url) => calls.push(`open ${url}`),
    pollMs: 2000,
  }
  return { gateway, calls }
}

it("signs in with one click, opens Boat, follows the browser, then disconnects", async () => {
  vi.useFakeTimers()
  const { gateway, calls } = fixture([{ state: "disconnected", signInAvailable: true }])
  await act(async () =>
    root.render(
      <BoatProvider gateway={gateway}>
        <BoatRow />
      </BoatProvider>,
    ),
  )
  // No token field anywhere: Sign in with Boat is the one way in.
  expect(host.querySelector("input")).toBeNull()
  expect(row()).toContain("Not connected")
  await act(async () => button("Sign in with Boat")?.click())
  expect(calls).toEqual(["read", "signIn", `open ${URL_}`])
  expect(row()).toContain("Finish signing in in your browser")
  // Open Boat again reopens the same page.
  await act(async () => button("Open Boat")?.click())
  expect(calls.at(-1)).toBe(`open ${URL_}`)
  // While the browser is out the row reads again on a timer, and hears it end.
  calls.length = 0
  const read = gateway.read
  gateway.read = async () => {
    calls.push("read")
    return signedIn
  }
  await act(async () => vi.advanceTimersByTime(2000))
  expect(calls).toContain("read")
  expect(row()).toContain("Connected as octo")
  gateway.read = read
  await act(async () => button("Disconnect")?.click())
  expect(calls).toContain("disconnect")
  expect(row()).toContain("Not connected")
})

it("cancels a sign-in still waiting for the browser", async () => {
  const { gateway, calls } = fixture([waiting])
  await act(async () =>
    root.render(
      <BoatProvider gateway={gateway}>
        <BoatRow />
      </BoatProvider>,
    ),
  )
  expect(row()).toContain("Finish signing in")
  await act(async () => button("Cancel")?.click())
  expect(calls).toContain("cancel")
  expect(button("Sign in with Boat")).toBeDefined()
})

it("says why a sign-in ended, and how a refused start reads", async () => {
  const { gateway } = fixture([
    {
      state: "disconnected",
      signInAvailable: true,
      signIn: { state: "failed", cause: "denied" },
    },
  ])
  gateway.signIn = async () => {
    throw new NessaBoatError(new Error("unclassified"))
  }
  await act(async () =>
    root.render(
      <BoatProvider gateway={gateway}>
        <BoatRow />
      </BoatProvider>,
    ),
  )
  expect(row()).toContain("Sign-in was cancelled on Boat.")
  await act(async () => button("Sign in with Boat")?.click())
  expect(row()).toContain("Boat sign-in could not start")
  expect(button("Disconnect")).toBeUndefined()
})

it("offers nothing to click on a build with no Boat app", async () => {
  const { gateway } = fixture([{ state: "disconnected", signInAvailable: false }])
  await act(async () =>
    root.render(
      <BoatProvider gateway={gateway}>
        <BoatRow />
      </BoatProvider>,
    ),
  )
  expect(button("Sign in with Boat")).toBeUndefined()
})

it("rests without a connected gateway", async () => {
  await act(async () => root.render(<BoatRow />))
  expect(button("Sign in with Boat")?.hasAttribute("disabled")).toBe(true)
})
