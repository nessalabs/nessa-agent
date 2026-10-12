// @vitest-environment jsdom
/**
 * Settings › Access native hooks: the rows rest without a gateway, and with
 * one they show the catalog's choice and send a change.
 */
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { NativeHooksGateway } from "../adapters/native-hooks-gateway"
import { NativeHooksProvider, NativeHooksRows } from "./native-hooks-rows"

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

const row = (id: string) => host.querySelector(`[data-setting="${id}"]`)
const toggle = (id: string) =>
  row(id)?.querySelector<HTMLButtonElement>('[role="switch"]')

it("rests both rows when this window has no gateway", async () => {
  await act(async () => root.render(<NativeHooksRows />))
  for (const id of ["claude-native-hooks", "codex-native-hooks"]) {
    expect(row(id)?.textContent).toContain("Not available in this window.")
    expect(row(id)?.getAttribute("data-disabled")).not.toBeNull()
    expect(toggle(id)).toBeNull()
  }
  expect(host.textContent).toContain(
    "A hook can deny or change a tool call before Nessa asks.",
  )
})

it("shows the catalog's choice and sends a change for that harness", async () => {
  const sent: { agent: string; enabled: boolean }[] = []
  let choice = { claude: false, codex: true }
  const gateway: NativeHooksGateway = {
    read: () => Promise.resolve({ ok: true, choice: { ...choice } }),
    set: (agent, enabled) => {
      sent.push({ agent, enabled })
      choice = { ...choice, [agent]: enabled }
      return Promise.resolve({ ok: true, agent, enabled })
    },
  }
  await act(async () =>
    root.render(
      <NativeHooksProvider gateway={gateway}>
        <NativeHooksRows />
      </NativeHooksProvider>,
    ),
  )
  await act(async () => {})
  expect(toggle("claude-native-hooks")?.getAttribute("aria-checked")).toBe("false")
  expect(toggle("codex-native-hooks")?.getAttribute("aria-checked")).toBe("true")
  await act(async () => toggle("claude-native-hooks")?.click())
  expect(sent).toEqual([{ agent: "claude", enabled: true }])
  expect(toggle("claude-native-hooks")?.getAttribute("aria-checked")).toBe("true")
})

it("keeps the previous choice when the gateway refuses the change", async () => {
  const gateway: NativeHooksGateway = {
    read: () => Promise.resolve({ ok: true, choice: { claude: false, codex: false } }),
    set: () => Promise.resolve({ ok: false }),
  }
  await act(async () =>
    root.render(
      <NativeHooksProvider gateway={gateway}>
        <NativeHooksRows />
      </NativeHooksProvider>,
    ),
  )
  await act(async () => {})
  await act(async () => toggle("codex-native-hooks")?.click())
  expect(toggle("codex-native-hooks")?.getAttribute("aria-checked")).toBe("false")
  expect(row("codex-native-hooks")?.textContent).toContain("Could not change this.")
})

it("shows the choice a lost response left in effect", async () => {
  let choice = { claude: false, codex: false }
  const gateway: NativeHooksGateway = {
    read: () => Promise.resolve({ ok: true, choice: { ...choice } }),
    set: (_agent, enabled) => {
      choice = { ...choice, codex: enabled }
      return Promise.resolve({ ok: false })
    },
  }
  await act(async () =>
    root.render(
      <NativeHooksProvider gateway={gateway}>
        <NativeHooksRows />
      </NativeHooksProvider>,
    ),
  )
  await act(async () => {})
  await act(async () => toggle("codex-native-hooks")?.click())
  expect(toggle("codex-native-hooks")?.getAttribute("aria-checked")).toBe("true")
  expect(row("codex-native-hooks")?.textContent).toContain("Could not change this.")
})
