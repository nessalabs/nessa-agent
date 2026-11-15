// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { Agent37Status, Cloud } from "@nessa/client"
import type { Agent37Gateway } from "../adapters/agent37-gateway"
import { Agent37Provider, Agent37Row } from "./agent37-row"

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
const button = (name: string) =>
  [...document.querySelectorAll("button")].find((each) => each.textContent === name)
const row = () => document.querySelector<HTMLElement>("[data-agent37-state]")
const choice = () => document.querySelector<HTMLElement>("[data-cloud-choice]")

it("connects with a key pasted in its dialog, keeps nothing of it, and disconnects", async () => {
  const calls: string[] = []
  let status: Agent37Status = { state: "disconnected" }
  let cloud: Cloud = "boat"
  const gateway: Agent37Gateway = {
    read: async () => status,
    save: async (key) => {
      calls.push(`save ${key}`)
      status = { state: "connected", template: "nessa-runtime@1" }
      return status
    },
    disconnect: async () => {
      calls.push("disconnect")
      status = { state: "disconnected" }
      cloud = "boat"
      return status
    },
    cloud: async () => ({ cloud }),
    chooseCloud: async (next) => {
      calls.push(`choose ${next}`)
      cloud = next
      return { cloud }
    },
  }
  await act(async () =>
    root.render(
      <Agent37Provider gateway={gateway}>
        <Agent37Row />
      </Agent37Provider>,
    ),
  )
  expect(row()?.dataset.agent37State).toBe("disconnected")
  expect(choice()?.dataset.cloudChoice).toBe("boat")
  expect(host.querySelector("input")).toBeNull()
  await act(async () => button("Connect…")?.click())
  const field = document.querySelector<HTMLInputElement>("[data-agent37-dialog] input")
  if (!field) throw new Error("missing key field")
  expect(field.type).toBe("password")
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(
      field,
      "  sk_live_fixture  ",
    )
    field.dispatchEvent(new Event("input", { bubbles: true }))
  })
  await act(async () => {
    document
      .querySelector("[data-agent37-dialog] form")
      ?.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }))
  })
  expect(calls).toEqual(["save sk_live_fixture"])
  expect(document.querySelector("[data-agent37-dialog]")).toBeNull()
  expect(row()?.dataset.agent37State).toBe("connected")
  expect(document.body.textContent).not.toContain("sk_live_fixture")
  await act(async () =>
    document
      .querySelector<HTMLButtonElement>('[data-agent37-action="disconnect"]')
      ?.click(),
  )
  expect(calls).toEqual(["save sk_live_fixture", "disconnect"])
  expect(row()?.dataset.agent37State).toBe("disconnected")
  expect(choice()?.dataset.cloudChoice).toBe("boat")
})
