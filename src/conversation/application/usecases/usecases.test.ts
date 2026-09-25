import { textContent } from "../../model"
import { describe, expect, it } from "vitest"
import { emptyLocalTabs } from "../local-tabs"
import { UNTITLED } from "../queries/roster"
import {
  beginSend,
  closeConversation,
  failSend,
  openConversation,
  openListed,
  setActive,
  setDraft,
} from "./index"
const identity = {
  conversationId: "c0",
  executionId: "execution",
  actionId: "action",
  mode: "queued" as const,
}

describe("local submission evidence", () => {
  it("preserves exact content and permits a second queued draft", () => {
    const content = textContent("  code\n ")
    const pending = beginSend(emptyLocalTabs(), { ...identity, content })
    // Named by the gateway, not here: the view carries its title.
    expect(pending.conversations[0]!.title).toBe("New chat")
    expect(pending.conversations[0]!.turns[0]).toMatchObject({
      from: "user",
      content,
      receipt: "sending",
      executionId: "execution",
      actionId: "action",
    })
    const second = beginSend(pending, {
      ...identity,
      executionId: "second",
      actionId: "second-action",
      content: textContent("next"),
    })
    expect(second.conversations[0]!.turns).toHaveLength(2)
  })
  it("records uncertain delivery on the affected user turn without inventing assistant output", () => {
    const pending = beginSend(emptyLocalTabs(), {
      ...identity,
      content: textContent("hello"),
    })
    const failed = failSend(pending, "c0", "execution", "offline", { kind: "uncertain" })
    expect(failed.conversations[0]!.turns).toHaveLength(1)
    expect(failed.conversations[0]!.turns[0]).toMatchObject({
      receipt: "unknown",
      error: "offline",
    })
  })
  /** A send refused before admission: the message was not taken, so the draft is back. */
  const refused = { kind: "refused", reupload: false } as const

  it("returns to idle after confirmed rejection without offering unknown-delivery retry", () => {
    const pending = beginSend(emptyLocalTabs(), {
      ...identity,
      content: textContent("hello"),
    })
    const failed = failSend(pending, "c0", "execution", "Agent not configured", refused)
    expect(failed.conversations[0]!.phase).toBe("idle")
    expect(failed.conversations[0]!.turns[0]).toMatchObject({
      receipt: "failed",
      error: "Agent not configured",
    })
  })
  it("does not consume an empty draft", () => {
    const tabs = emptyLocalTabs()
    expect(beginSend(tabs, { ...identity, content: textContent(" ") })).toBe(tabs)
  })
})

describe("openConversation / closeConversation", () => {
  it("keeps conversation ids off the turn counter", () => {
    const opened = openConversation(emptyLocalTabs())
    expect(opened.conversations.map((item) => item.id)).toEqual(["c0", "c1"])
    expect(opened.activeId).toBe("c1")
  })

  it("never empties the tabs", () => {
    const only = closeConversation(emptyLocalTabs(), "c0")
    expect(only.conversations).toHaveLength(1)
    expect(only.conversations[0]!.id).toBe("c1")
    expect(only.activeId).toBe("c1")
  })

  it("no-ops an unknown id", () => {
    const tabs = emptyLocalTabs()
    const next = closeConversation(tabs, "missing")
    expect(next.conversations).toBe(tabs.conversations)
    expect(next.activeId).toBe("c0")
  })
})

describe("setActive / setDraft", () => {
  it("switches tabs and writes a draft on the open conversation", () => {
    const two = openConversation(emptyLocalTabs())
    const drafted = setDraft(two, { draft: textContent("note"), id: "c0" })
    expect(setActive(drafted, "c0").activeId).toBe("c0")
    expect(drafted.conversations[0]!.draft).toEqual(textContent("note"))
    expect(setActive(two, "missing")).toBe(two)
  })
})

describe("openListed", () => {
  const listed = {
    serverConversationId: "0b8f1c2e-1111-4a4a-8b8b-000000000001",
    title: "Flights to Lisbon",
  }

  it("reopens a listed conversation as a ready tab named by the list", () => {
    const opened = openListed(emptyLocalTabs(), listed)
    const tab = opened.conversations.find((item) => item.id === opened.activeId)!
    expect(opened.conversations).toHaveLength(2)
    expect(tab).toMatchObject({
      title: "Flights to Lisbon",
      serverConversationId: listed.serverConversationId,
      serverReady: true,
      turns: [],
    })
    expect(tab.titleEdited).toBeUndefined()
  })

  it("shows the tab that already holds it rather than opening a second", () => {
    const once = openListed(emptyLocalTabs(), listed)
    const elsewhere = setActive(once, "c0")
    const twice = openListed(elsewhere, { ...listed, title: null })
    expect(twice.conversations).toHaveLength(2)
    expect(twice.activeId).toBe(once.activeId)
  })

  it("names a conversation the gateway has no title for as its list row does", () => {
    const opened = openListed(emptyLocalTabs(), { ...listed, title: null })
    expect(opened.conversations.at(-1)!.title).toBe(UNTITLED)
  })
})
