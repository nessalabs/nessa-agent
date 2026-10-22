import { expect, it } from "vitest"
import { focusedPane, panesOf } from "../../../split-panes/model/pane-layout"
import { roomyGrid, shownBy, testIndex } from "../../testing"
import { initialWorkspace, paneIds, shownIds } from "../workspace-state"
import { createDraft, openBeside, openSession } from "./panes"
import { indexLoaded, sessionRemoved } from "./updates"
import { closeSessionTab } from "./session-tabs"

const loaded = () =>
  indexLoaded(initialWorkspace, { index: testIndex(), draftId: "draft", read: "r" })

it("keeps displaced documents in tabs without treating hidden tabs as visible conversations", () => {
  const next = openSession(loaded(), { sessionId: "b" })
  expect(next.sessionTabs).toEqual(["a", "b"])
  expect(paneIds(next.panes)).toEqual(["b"])
  expect(shownIds(next).has("a")).toBe(false)
})

it("switches the focused split's document without changing either split's size or its neighbor", () => {
  const two = openBeside(loaded(), { sessionId: "b", room: roomyGrid })
  const next = openSession(two, { sessionId: "c" })
  expect(paneIds(next.panes)).toEqual(["a", "c"])
  expect(next.panes?.columns.map((c) => c.share)).toEqual(
    two.panes?.columns.map((c) => c.share),
  )
  expect(next.panes?.columns[0]).toBe(two.panes?.columns[0])
  expect(next.sessionTabs).toEqual(["a", "b", "c"])
})

it("selecting a visible document focuses its existing pane instead of replacing another split", () => {
  const two = openBeside(loaded(), { sessionId: "b", room: roomyGrid })
  const next = openSession(two, { sessionId: "a" })
  expect(paneIds(next.panes)).toEqual(["a", "b"])
  expect(shownBy(focusedPane(next.panes!))).toBe("a")
})

it("closing a visible tab reuses its split for a hidden document and leaves its neighbor alone", () => {
  const two = openBeside(loaded(), { sessionId: "b", room: roomyGrid })
  const three = openSession(two, { sessionId: "c" })
  const next = closeSessionTab(three, { sessionId: "c", draftId: "draft" })
  expect(paneIds(next.panes)).toEqual(["a", "b"])
  expect(next.panes?.columns[0]).toBe(three.panes?.columns[0])
  expect(next.sessionTabs).toEqual(["a", "b"])
})

it("closing a hidden tab does not touch the split arrangement", () => {
  const state = openSession(loaded(), { sessionId: "b" })
  const next = closeSessionTab(state, { sessionId: "a", draftId: "draft" })
  expect(next.panes).toBe(state.panes)
  expect(next.sessionTabs).toEqual(["b"])
  expect(closeSessionTab(next, { sessionId: "missing", draftId: "draft" })).toBe(next)
})

it("closing the last tab leaves one new conversation home, and deleting a session removes its tab", () => {
  const state = openSession(loaded(), { sessionId: "b" })
  const gone = sessionRemoved(state, { sessionId: "a", revision: 2, draftId: "unused" })
  expect(gone.sessionTabs).not.toContain("a")
  const next = closeSessionTab(gone, { sessionId: "b", draftId: "draft" })
  expect(next.sessionTabs).toEqual(["draft"])
  expect(panesOf(next.panes!)).toHaveLength(1)
  expect(next.drafts).toHaveProperty("draft")
})

it("retains an unsent draft across tab switches and releases it only when its tab closes", () => {
  const drafted = createDraft(loaded(), { draftId: "unsent" })
  const typed = { ...drafted, composerText: { unsent: "Keep this thought" } }
  const switched = openSession(typed, { sessionId: "b" })
  expect(switched.composerText.unsent).toBe("Keep this thought")
  const back = openSession(switched, { sessionId: "unsent" })
  expect(paneIds(back.panes)).toEqual(["unsent"])
  const closed = closeSessionTab(back, { sessionId: "unsent", draftId: "fresh" })
  expect(closed.drafts.unsent).toBeUndefined()
  expect(closed.composerText.unsent).toBeUndefined()
  expect(closed.sessionTabs).not.toContain("unsent")
})

it("closing the last unsent tab shows a fresh home without leaving a pane pointing at a missing draft", () => {
  const drafted = createDraft(loaded(), { draftId: "unsent" })
  const alone = closeSessionTab(drafted, { sessionId: "a", draftId: "unused" })
  const next = closeSessionTab(alone, { sessionId: "unsent", draftId: "fresh" })
  expect(paneIds(next.panes)).toEqual(["fresh"])
  expect(next.sessionTabs).toEqual(["fresh"])
  expect(next.drafts.unsent).toBeUndefined()
  expect(next.drafts.fresh).toBeDefined()
})
