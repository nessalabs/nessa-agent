// @vitest-environment jsdom
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import { PanelOwnerAsk } from "./owner-ask"

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

const ask = {
  askId: "0123456789abcdef0123456789abcdef",
  question: "Ship it today?",
  choices: [
    { label: "Ship", consequence: "It goes out today." },
    { label: "Wait", consequence: "It waits for Monday." },
  ],
  recommended: 0,
  reason: "Every check passed.",
  askedAtMs: 1,
}

const choice = (label: string): HTMLElement => {
  const found = container.querySelector<HTMLElement>(`[data-owner-ask-choice="${label}"]`)
  if (!found) throw new Error(`no choice ${label}`)
  return found
}

it("offers every choice again when the gateway did not take the answer", async () => {
  const sent: string[] = []
  let settle!: (taken: boolean) => void
  act(() => {
    root.render(
      <PanelOwnerAsk
        ask={ask}
        submit={(text) => {
          sent.push(text)
          return new Promise((resolve) => (settle = resolve))
        }}
        raiseAccess={() => Promise.resolve(true)}
        access="ask"
        working={false}
      />,
    )
  })
  act(() => choice("Wait").click())
  expect(sent).toEqual(["Wait"])
  expect(container.querySelector("[aria-busy='true']")).not.toBeNull()
  await act(async () => settle(false))
  expect(container.querySelector("[data-owner-ask-failure]")?.textContent).toContain(
    "was not sent",
  )
  expect(container.querySelector("[aria-busy='true']")).toBeNull()
  act(() => choice("Ship").click())
  expect(sent).toEqual(["Wait", "Ship"])
})

const access = {
  askId: "fedcba9876543210fedcba9876543210",
  question: "Allow full access in this chat?",
  choices: [
    { label: "Approve", consequence: "Raises this chat to Full access." },
    { label: "Not now", consequence: "Keeps this chat's access as it is." },
  ],
  recommended: 1,
  reason: "The toolchain installs outside the project.",
  askedAtMs: 1,
  access: {
    mode: "full" as const,
    grants: [
      "Agents in this chat can run any command",
      "They won't ask you before they act",
      "You can undo this from Access, under the message box",
    ],
  },
}

const button = (name: string): HTMLButtonElement => {
  const found = [...container.querySelectorAll<HTMLButtonElement>("button")].find(
    (each) => each.textContent === name,
  )
  if (!found) throw new Error(`no button ${name}`)
  return found
}

it("sends Approve only once the chat runs at the access asked for", async () => {
  const steps: string[] = []
  const render = (
    shown: typeof access | undefined,
    { working = false, mode }: { working?: boolean; mode?: "ask" | "full" } = {},
  ) =>
    act(() => {
      root.render(
        <PanelOwnerAsk
          ask={shown}
          submit={(text) => {
            steps.push(`sent ${text}`)
            return Promise.resolve(true)
          }}
          raiseAccess={(raised) => {
            steps.push(`raised ${raised}`)
            return Promise.resolve(true)
          }}
          access={mode}
          working={working}
        />,
      )
    })
  // While the agent works, Approve waits and says why.
  render(access, { working: true, mode: "ask" })
  expect(container.querySelector("[data-access-request='full']")).not.toBeNull()
  expect(button("Approve").getAttribute("aria-disabled")).toBe("true")
  expect(container.textContent).toContain("once the agent stops")
  render(access, { mode: "ask" })
  expect(container.querySelector("[data-slot=access-request-grants]")).toBeNull()
  await act(async () => button("Approve").click())
  // The gateway took the change, but the chat does not run at it yet.
  expect(steps).toEqual(["raised full"])
  render(access)
  expect(steps).toEqual(["raised full"])
  await act(async () => render(access, { mode: "full" }))
  expect(steps).toEqual(["raised full", "sent Approve"])
  // The list lets the ask go; the card stays to say what was allowed.
  render(undefined, { mode: "full" })
  const grants = [...container.querySelectorAll("[data-slot=access-request-grants] li")]
  expect(grants.map((item) => item.textContent)).toEqual(access.access.grants)
  expect(container.querySelector("button")).toBeNull()
})

it("sends no Approve when the ask left before the access changed", async () => {
  const steps: string[] = []
  const render = (shown: typeof access | undefined, mode: "ask" | "full") =>
    act(() => {
      root.render(
        <PanelOwnerAsk
          ask={shown}
          submit={(text) => {
            steps.push(`sent ${text}`)
            return Promise.resolve(true)
          }}
          raiseAccess={() => Promise.resolve(true)}
          access={mode}
          working={false}
        />,
      )
    })
  render(access, "ask")
  await act(async () => button("Approve").click())
  // The person answered in words; then the access arrives.
  render(undefined, "ask")
  await act(async () => render(undefined, "full"))
  expect(steps).toEqual([])
})

it("sends no Approve when the access did not change, and Not now changes nothing", async () => {
  const steps: string[] = []
  act(() => {
    root.render(
      <PanelOwnerAsk
        ask={access}
        submit={(text) => {
          steps.push(`sent ${text}`)
          return Promise.resolve(true)
        }}
        raiseAccess={() => {
          steps.push("raise refused")
          return Promise.resolve(false)
        }}
        access="ask"
        working={false}
      />,
    )
  })
  await act(async () => button("Approve").click())
  expect(steps).toEqual(["raise refused"])
  expect(container.textContent).toContain("Access didn’t change.")
  await act(async () => button("Not now").click())
  expect(steps).toEqual(["raise refused", "sent Not now"])
})
