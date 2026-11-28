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

/** The choice a role's menu shows. */
function chosen(role: ReplyStyleRole) {
  return host
    .querySelector(`[data-reply-style-role="${role}"] [data-choice-trigger]`)
    ?.getAttribute("data-choice")
}

/** Types into the writer that "Your own" opens (a dialog, outside the row). */
async function write(role: ReplyStyleRole, text: string) {
  const field = document.querySelector<HTMLTextAreaElement>(
    `[data-reply-style-text="${role}"]`,
  )
  if (!field) throw new Error("missing writer")
  await act(async () => {
    const set = Object.getOwnPropertyDescriptor(
      HTMLTextAreaElement.prototype,
      "value",
    )?.set
    set?.call(field, text)
    field.dispatchEvent(new Event("input", { bubbles: true }))
  })
}

it("the Nessa level starts plain, and off saves as it is chosen", async () => {
  const { gateway, saves } = memory()
  await act(async () => root.render(<ReplyStyleControl gateway={gateway} />))
  expect(chosen("thread")).toBe("plain")
  expect(
    host.querySelector('[data-reply-style-role="thread"] [data-value="default"]'),
  ).toBeNull()
  await pick("thread", "off")
  expect(saves).toEqual([["thread", { mode: "off" }, undefined]])
  expect(chosen("thread")).toBe("off")
})

it("your own words save only on Save, trimmed, and a channel can follow Nessa again", async () => {
  const { gateway, saves } = memory()
  await act(async () =>
    root.render(<ReplyStyleControl channelId="ch_aaaaaaaaaa" gateway={gateway} />),
  )
  expect(chosen("coordinator")).toBe("default")
  // Your own opens the writer, and the row keeps what is saved until Save.
  await pick("coordinator", "custom")
  expect(saves).toEqual([])
  expect(chosen("coordinator")).toBe("default")
  await write("coordinator", "  Never this.  ")
  // Cancel saves nothing and closes the writer.
  await act(async () =>
    [...document.querySelectorAll<HTMLButtonElement>("button")]
      .find((each) => each.textContent === "Cancel")
      ?.click(),
  )
  expect(saves).toEqual([])
  expect(document.querySelector('[data-reply-style-writer="coordinator"]')).toBeNull()
  // A role not on your own words has nothing to view.
  expect(host.querySelector('[data-reply-style-view="coordinator"]')).toBeNull()
  await pick("coordinator", "custom")
  await write("coordinator", "  One line only.  ")
  await act(async () =>
    document
      .querySelector<HTMLButtonElement>('[data-reply-style-save="coordinator"]')
      ?.click(),
  )
  expect(saves).toEqual([
    ["coordinator", { mode: "custom", text: "One line only." }, "ch_aaaaaaaaaa"],
  ])
  expect(chosen("coordinator")).toBe("custom")
  expect(document.querySelector('[data-reply-style-writer="coordinator"]')).toBeNull()
  // View opens the writer on the words saved.
  await act(async () =>
    host
      .querySelector<HTMLButtonElement>('[data-reply-style-view="coordinator"]')
      ?.click(),
  )
  expect(
    document.querySelector<HTMLTextAreaElement>('[data-reply-style-text="coordinator"]')
      ?.value,
  ).toBe("One line only.")
  await act(async () =>
    [...document.querySelectorAll<HTMLButtonElement>("button")]
      .find((each) => each.textContent === "Cancel")
      ?.click(),
  )
  await pick("coordinator", "default")
  expect(saves.at(-1)).toEqual(["coordinator", undefined, "ch_aaaaaaaaaa"])
  expect(chosen("coordinator")).toBe("default")
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
