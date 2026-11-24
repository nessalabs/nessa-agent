// @vitest-environment jsdom
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"
import { afterEach, beforeEach, expect, it } from "vitest"
import type { ReplyStyleChoice, ReplyStyleRole, ReplyStyles } from "@nessa/client"
import type { ReplyStyleGateway } from "../adapters/reply-style-gateway"
import { ReplyStyleControl } from "./reply-style-control"

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

/** A gateway that keeps choices in memory and records each save. */
function memory() {
  const saves: [ReplyStyleRole, ReplyStyleChoice | undefined, string | undefined][] = []
  let nessa: ReplyStyles["nessa"] = {}
  let channel: ReplyStyles["nessa"] = {}
  const read = (channelId?: string): ReplyStyles => {
    const pick = (role: ReplyStyleRole) =>
      (channelId === undefined ? undefined : channel[role]) ??
      nessa[role] ?? { mode: "plain" }
    return {
      ...(channelId === undefined ? {} : { channelId, channel }),
      nessa,
      effective: { coordinator: pick("coordinator"), thread: pick("thread") },
    }
  }
  const gateway: ReplyStyleGateway = {
    read: async (channelId) => read(channelId),
    set: async (role, choice, channelId) => {
      saves.push([role, choice, channelId])
      const level = { ...(channelId === undefined ? nessa : channel) }
      if (choice === undefined) delete level[role]
      else level[role] = choice
      if (channelId === undefined) nessa = level
      else channel = level
      return read(channelId)
    },
  }
  return { gateway, saves }
}

async function pick(role: ReplyStyleRole, value: string) {
  const button = host.querySelector<HTMLButtonElement>(
    `[data-reply-style-role="${role}"] [data-choice-trigger]`,
  )
  if (!button) throw new Error(`missing ${role} menu`)
  await act(async () => {
    button.focus()
    button.dispatchEvent(
      new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }),
    )
  })
  const option = document.querySelector<HTMLElement>(
    `[role="menuitemradio"][data-value="${value}"]`,
  )
  if (!option) throw new Error(`missing choice ${value}`)
  await act(async () => option.click())
}

function detail(role: ReplyStyleRole) {
  return host.querySelector(`[data-reply-style-role="${role}"]`)?.textContent ?? ""
}

it("the Nessa level starts plain, and off saves as it is chosen", async () => {
  const { gateway, saves } = memory()
  await act(async () => root.render(<ReplyStyleControl gateway={gateway} />))
  expect(detail("thread")).toContain("Nessa’s plain style")
  expect(
    host.querySelector('[data-reply-style-role="thread"] [data-value="default"]'),
  ).toBeNull()
  await pick("thread", "off")
  expect(saves).toEqual([["thread", { mode: "off" }, undefined]])
  expect(detail("thread")).toContain("No style")
})

it("your own words save only on Save, trimmed, and a channel can follow Nessa again", async () => {
  const { gateway, saves } = memory()
  await act(async () =>
    root.render(<ReplyStyleControl channelId="ch_aaaaaaaaaa" gateway={gateway} />),
  )
  expect(detail("coordinator")).toContain("Same as Nessa")
  await pick("coordinator", "custom")
  expect(saves).toEqual([])
  const field = host.querySelector<HTMLTextAreaElement>(
    '[data-reply-style-text="coordinator"]',
  )
  if (!field) throw new Error("missing text field")
  await act(async () => {
    const set = Object.getOwnPropertyDescriptor(
      HTMLTextAreaElement.prototype,
      "value",
    )?.set
    set?.call(field, "  One line only.  ")
    field.dispatchEvent(new Event("input", { bubbles: true }))
  })
  await act(async () =>
    host
      .querySelector<HTMLButtonElement>('[data-reply-style-save="coordinator"]')
      ?.click(),
  )
  expect(saves).toEqual([
    ["coordinator", { mode: "custom", text: "One line only." }, "ch_aaaaaaaaaa"],
  ])
  await pick("coordinator", "default")
  expect(saves.at(-1)).toEqual(["coordinator", undefined, "ch_aaaaaaaaaa"])
  expect(detail("coordinator")).toContain("Same as Nessa: Nessa’s plain style")
})

it("a read that fails says so and offers Reload", async () => {
  const gateway: ReplyStyleGateway = {
    read: async () => {
      throw new Error("down")
    },
    set: async () => {
      throw new Error("unexpected save")
    },
  }
  await act(async () => root.render(<ReplyStyleControl gateway={gateway} />))
  expect(host.querySelector('[role="alert"]')?.textContent).toContain(
    "Couldn’t read this.",
  )
})
