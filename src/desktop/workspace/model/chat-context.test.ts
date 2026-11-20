import { describe, expect, it } from "vitest"
import { chatContextTab, contextFirst, isChatContextTab, panelTabs } from "./chat-context"
import { closeInPanel, openInPanel } from "./chat-panel"

const notes = { plugin: "sample", id: "notes" }
const trail = { plugin: "sample", id: "trail" }

describe("Chat context's place in a chat's panel", () => {
  it("is the only tab, in front, while nothing else is open", () => {
    const context = chatContextTab("a")
    expect(panelTabs("a", undefined)).toEqual({
      tabs: [context],
      front: context,
      widgets: [],
    })
    expect(isChatContextTab(context)).toBe(true)
    expect(isChatContextTab(notes)).toBe(false)
  })

  it("comes first, before every widget, and leaves the front to the widget opened last", () => {
    const panel = openInPanel(openInPanel(undefined, notes), trail)
    expect(panelTabs("a", panel)).toEqual({
      tabs: [chatContextTab("a"), notes, trail],
      front: trail,
      widgets: [notes, trail],
    })
  })

  it("is in front once chosen, and stays first where it is stored", () => {
    const chosen = contextFirst(
      openInPanel(openInPanel(undefined, notes), chatContextTab("a")),
    )
    expect(chosen.tabs).toEqual([chatContextTab("a"), notes])
    expect(panelTabs("a", chosen).front).toEqual(chatContextTab("a"))
    // A widget closed brings the tab before it to the front, down to Chat context.
    const closed = closeInPanel(contextFirst(openInPanel(chosen, trail)), trail)
    expect(closed?.front).toEqual(notes)
    expect(closed && panelTabs("a", closeInPanel(closed, notes)).front).toEqual(
      chatContextTab("a"),
    )
  })

  it("is moved first in storage when it was stored after widgets", () => {
    const stored = { tabs: [notes, chatContextTab("a"), trail], front: trail }
    expect(contextFirst(stored)).toEqual({
      tabs: [chatContextTab("a"), notes, trail],
      front: trail,
    })
    const already = contextFirst(stored)
    expect(contextFirst(already)).toBe(already)
  })
})
