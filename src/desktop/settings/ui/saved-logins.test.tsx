// @vitest-environment jsdom
/**
 * Saved logins: Settings lists one row per agent, saves a pasted login and
 * clears the field, says only "Saved" afterwards with Remove, and says why a
 * change was refused; a paired computer's switch in Devices sends its
 * environment and reads back what the gateway answered.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { LoginAgent } from "@nessa/client"
import type {
  SavedLogins,
  SavedLoginsGateway,
  SavedLoginsOutcome,
} from "../adapters/saved-logins-gateway"
import { DeviceLoginsSwitch, SavedLoginsGroup, SavedLoginsProvider } from "./saved-logins"

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

const peer = `peer:${"ef".repeat(32)}`

function fake() {
  let logins: SavedLogins = {
    saved: { claude: false, codex: false, opencode: false },
    devicesOn: new Set(),
  }
  const calls: unknown[][] = []
  let refuse: string | undefined
  const answer = (): Promise<SavedLoginsOutcome> =>
    Promise.resolve(
      refuse ? { kind: "refused", sentence: refuse } : { kind: "read", logins },
    )
  const gateway: SavedLoginsGateway = {
    read: answer,
    save: (agent: LoginAgent, login: string) => {
      calls.push(["save", agent, login])
      if (!refuse) logins = { ...logins, saved: { ...logins.saved, [agent]: true } }
      return answer()
    },
    remove: (agent: LoginAgent) => {
      calls.push(["remove", agent])
      logins = { ...logins, saved: { ...logins.saved, [agent]: false } }
      return answer()
    },
    setDevice: (environment: string, on: boolean) => {
      calls.push(["setDevice", environment, on])
      const devicesOn = new Set(logins.devicesOn)
      if (on) devicesOn.add(environment)
      else devicesOn.delete(environment)
      logins = { ...logins, devicesOn }
      return answer()
    },
  }
  return { gateway, calls, refuseWith: (sentence: string) => (refuse = sentence) }
}

function need<T>(value: T | null | undefined): T {
  if (value === null || value === undefined) throw new Error("not on the page")
  return value
}

async function type(input: HTMLInputElement, value: string) {
  const setter = need(
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set,
  )
  await act(async () => {
    setter.call(input, value)
    input.dispatchEvent(new Event("input", { bubbles: true }))
  })
}

const input = (agent: string) =>
  need(host.querySelector<HTMLInputElement>(`[data-saved-login-input="${agent}"]`))
const save = (agent: string) =>
  need(host.querySelector<HTMLButtonElement>(`[data-saved-login-save="${agent}"]`))
const row = (agent: string) => need(host.querySelector(`[data-saved-login="${agent}"]`))

it("saves a pasted login, then shows it only as saved, and removes it", async () => {
  const { gateway, calls } = fake()
  await act(async () =>
    root.render(
      <SavedLoginsProvider gateway={gateway}>
        <SavedLoginsGroup />
      </SavedLoginsProvider>,
    ),
  )
  expect(host.querySelectorAll("[data-saved-login]").length).toBe(3)
  await type(input("codex"), "  sk-codex  ")
  await act(async () => save("codex").click())
  expect(calls).toEqual([["save", "codex", "sk-codex"]])
  expect(input("codex").value).toBe("")
  expect(row("codex").querySelector('[data-saved-login-state="saved"]')).not.toBeNull()
  expect(host.textContent).not.toContain("sk-codex")
  expect(save("codex").textContent).toBe("Replace")
  const remove = need(
    [...row("codex").querySelectorAll("button")].find(
      (button) => button.textContent === "Remove",
    ),
  )
  await act(async () => remove.click())
  expect(calls.at(-1)).toEqual(["remove", "codex"])
  expect(row("codex").querySelector('[data-saved-login-state="saved"]')).toBeNull()
})

it("keeps what was typed and says why when the gateway refuses", async () => {
  const { gateway, refuseWith } = fake()
  await act(async () =>
    root.render(
      <SavedLoginsProvider gateway={gateway}>
        <SavedLoginsGroup />
      </SavedLoginsProvider>,
    ),
  )
  refuseWith("Unlock Nessa to change saved logins.")
  await type(input("claude"), "sk-ant-oat")
  await act(async () => save("claude").click())
  expect(input("claude").value).toBe("sk-ant-oat")
  expect(host.querySelector('[role="alert"]')?.textContent).toBe(
    "Unlock Nessa to change saved logins.",
  )
})

it("rests without a gateway", async () => {
  await act(async () =>
    root.render(
      <SavedLoginsProvider gateway={undefined}>
        <SavedLoginsGroup />
        <dl>
          <DeviceLoginsSwitch environment={peer} />
        </dl>
      </SavedLoginsProvider>,
    ),
  )
  expect(input("claude").disabled).toBe(true)
  expect(host.querySelector("[data-device-logins]")).toBeNull()
})

it("turns a paired computer's switch on and off", async () => {
  const { gateway, calls } = fake()
  await act(async () =>
    root.render(
      <SavedLoginsProvider gateway={gateway}>
        <dl>
          <DeviceLoginsSwitch environment={peer} />
        </dl>
      </SavedLoginsProvider>,
    ),
  )
  const fact = () => need(host.querySelector("[data-device-logins]"))
  const flip = () =>
    need(fact().querySelector<HTMLButtonElement>('[role="switch"]')).click()
  expect(fact().getAttribute("data-device-logins")).toBe("off")
  await act(async () => flip())
  expect(calls).toEqual([["setDevice", peer, true]])
  expect(fact().getAttribute("data-device-logins")).toBe("on")
  await act(async () => flip())
  expect(calls.at(-1)).toEqual(["setDevice", peer, false])
  expect(fact().getAttribute("data-device-logins")).toBe("off")
})
