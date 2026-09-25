import { expect, it } from "vitest"

import { conversation, type Conversation } from "../../model"
import type { ConversationSummary } from "../view"
import { matchesRosterQuery, rosterRows, rosterSelection, UNTITLED } from "./roster"

const lisbon = "0b8f1c2e-1111-4a4a-8b8b-000000000001"
const groceries = "0b8f1c2e-1111-4a4a-8b8b-000000000002"
const fresh = "0b8f1c2e-1111-4a4a-8b8b-000000000003"
const none = new Set<string>()

function listed(
  id: string,
  extra: Partial<ConversationSummary> = {},
): ConversationSummary {
  return {
    conversationId: id,
    title: "Flights to Lisbon",
    preview: "Done. Confirmation K7Q2PX is in your inbox.",
    updatedAtMs: 2,
    running: false,
    archived: false,
    ...extra,
  }
}

function tab(
  id: string,
  serverConversationId?: string,
  extra: Partial<Conversation> = {},
) {
  return { ...conversation(id), serverConversationId, ...extra } as Conversation
}

it("shows the gateway's title and preview, and reopens a conversation no tab holds", () => {
  expect(rosterRows([tab("c0")], [listed(lisbon)], none, "")).toEqual([
    {
      id: lisbon,
      title: "Flights to Lisbon",
      preview: "Done. Confirmation K7Q2PX is in your inbox.",
      target: { serverConversationId: lisbon, title: "Flights to Lisbon" },
    },
  ])
})

it("selects the tab that already holds a listed conversation", () => {
  const rows = rosterRows([tab("c3", lisbon)], [listed(lisbon)], none, "")
  expect(rows[0]!.target).toEqual({ tabId: "c3" })
})

it("keeps a name somebody gave the tab, and the gateway's otherwise", () => {
  const renamed = tab("c1", lisbon, { title: "Trip", titleEdited: true })
  const derived = tab("c2", groceries, { title: "Add oat milk" })
  const rows = rosterRows(
    [renamed, derived],
    [listed(lisbon), listed(groceries, { title: "Groceries" })],
    none,
    "",
  )
  expect(rows.map((row) => row.title)).toEqual(["Trip", "Groceries"])
})

it("names a conversation the gateway has no title for, and leaves its preview out", () => {
  const rows = rosterRows([], [listed(lisbon, { title: null, preview: null })], none, "")
  expect(rows[0]).toMatchObject({ title: UNTITLED, preview: undefined })
})

const said = [
  {
    id: "t",
    from: "user" as const,
    receipt: "delivered" as const,
    content: [{ type: "text" as const, text: "hi" }],
  },
]

it("leads with open conversations the list does not name yet, and skips unbound tabs", () => {
  const rows = rosterRows(
    [tab("c0"), tab("c4", fresh, { title: "Just now", turns: said })],
    [listed(lisbon)],
    none,
    "",
  )
  expect(rows.map((row) => [row.id, row.title])).toEqual([
    [fresh, "Just now"],
    [lisbon, "Flights to Lisbon"],
  ])
})

it("leaves out a restored tab this window has not read, and names a new one as a row does", () => {
  // Unread after a reload: it may name a conversation deleted elsewhere, or one
  // nothing was said in, and "New chat" would be a third name for either.
  expect(rosterRows([tab("c5", fresh)], [], none, "")).toEqual([])
  const unnamed = rosterRows([tab("c6", fresh, { turns: said })], [], none, "")
  expect(unnamed[0]!.title).toBe(UNTITLED)
})

it("leaves out an open conversation the gateway archived, or that was deleted", () => {
  // Otherwise archiving the conversation somebody has open would move it to
  // the top of the list rather than out of it.
  const archivedTab = tab("c1", lisbon, { turns: said })
  const deletedTab = tab("c2", groceries, { readError: "deleted", turns: said })
  expect(rosterRows([archivedTab, deletedTab], [], new Set([lisbon]), "")).toEqual([])
})

it("narrows by title or preview, regardless of case", () => {
  const rows = [
    listed(lisbon),
    listed(groceries, { title: "Groceries", preview: "And lemons" }),
  ]
  expect(rosterRows([], rows, none, "LEMON").map((row) => row.id)).toEqual([groceries])
  expect(rosterRows([], rows, none, "groc").map((row) => row.id)).toEqual([groceries])
  expect(rosterRows([], rows, none, "  ").map((row) => row.id)).toEqual([
    lisbon,
    groceries,
  ])
  expect(matchesRosterQuery({ title: "Tabs" }, "sidebar")).toBe(false)
})

it("marks the row for the active tab by the gateway's identity", () => {
  expect(rosterSelection([tab("c0"), tab("c1", lisbon)], "c1")).toBe(lisbon)
  expect(rosterSelection([tab("c0")], "c0")).toBeNull()
})

it("keeps a name somebody chose even when it is the default one", () => {
  const renamed = tab("c7", fresh, { title: "New chat", titleEdited: true, turns: said })
  expect(rosterRows([renamed], [], none, "")[0]!.title).toBe("New chat")
})

it("leaves out a row the gateway listed while also listing it as archived", () => {
  // Two reads a moment apart can each catch one side of an archive.
  expect(rosterRows([], [listed(lisbon)], new Set([lisbon]), "")).toEqual([])
})
