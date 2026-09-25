// @vitest-environment jsdom
import * as React from "react"
import { createRoot, type Root } from "react-dom/client"
import { Provider } from "react-redux"
import { afterEach, beforeEach, expect, it, vi } from "vitest"

import { createDependencies } from "../../composition/dependencies"
import { sessionReady } from "../../session/testing"
import { makeStore } from "../../store"
import { ControlFailedError, ConversationReadFailedError } from "../application/ports"
import { listConversations } from "../adapters/store/history"
import { openListed, refreshConversation } from "../adapters/store/slice"
import { scenarioEffects } from "../testing"
import { ConversationList } from "./conversation-list"
import type { RosterTarget } from "../application/queries/roster"

const written = "0b8f1c2e-1111-4a4a-8b8b-000000000001"
const other = "0b8f1c2e-1111-4a4a-8b8b-000000000003"

let container: HTMLDivElement
let root: Root

beforeEach(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  // jsdom has no `matchMedia`, and each row's avatar asks it about motion.
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia
  container = document.createElement("div")
  document.body.appendChild(container)
  root = createRoot(container)
})

afterEach(async () => {
  await React.act(async () => root.unmount())
  container.remove()
})

/** A gateway holding one conversation that no tab in this window has open. */
async function gatewayWithClosedConversation(
  overrides: Partial<ReturnType<typeof scenarioEffects>> = {},
) {
  const effects = scenarioEffects("echo")
  await effects.create(written)
  await effects.send({
    conversationId: written,
    executionId: "e1",
    actionId: "a1",
    text: "Book the   Lisbon flight",
    attachments: [],
    files: [],
  })
  const list = vi.fn(effects.list)
  const store = makeStore(
    createDependencies({ conversation: { ...effects, list, ...overrides } }),
  )
  return { store, list, effects }
}

async function render(store: ReturnType<typeof makeStore>, onSelect = vi.fn()) {
  await React.act(async () => {
    root.render(
      React.createElement(Provider, {
        store,
        children: React.createElement(ConversationList, { onSelect, onNew: () => {} }),
      }),
    )
  })
  return onSelect
}

function rows() {
  return [...container.querySelectorAll("[data-slot=conversation-history-item]")]
}

it("lists what the gateway holds, including a conversation no tab has open", async () => {
  const { store, list } = await gatewayWithClosedConversation()
  const onSelect = await render(store)
  // Once for the rows and once for which are archived.
  expect(list.mock.calls).toEqual([[false], [true]])
  expect(rows().map((row) => row.textContent)).toEqual([
    expect.stringContaining("Book the Lisbon flight"),
  ])
  await React.act(async () => (rows()[0] as HTMLButtonElement).click())
  expect(onSelect).toHaveBeenCalledWith({
    serverConversationId: written,
    title: null,
  } satisfies RosterTarget)
})

it("reads the list again once the gateway arrives", async () => {
  const { store, list } = await gatewayWithClosedConversation()
  await render(store)
  await React.act(async () => {
    store.dispatch(
      sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
    )
  })
  expect(list).toHaveBeenCalledTimes(4)
})

it("says the list could not be loaded rather than that there is nothing", async () => {
  const effects = scenarioEffects("echo")
  const list = vi.fn(async () => {
    throw new ConversationReadFailedError("unavailable")
  })
  const store = makeStore(createDependencies({ conversation: { ...effects, list } }))
  await render(store)
  // Before the gateway arrives there is nothing to have failed yet.
  expect(container.textContent).toContain("Connecting to the gateway")
  await React.act(async () => {
    store.dispatch(
      sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
    )
  })
  expect(rows()).toHaveLength(0)
  expect(container.textContent).toContain("Conversations could not be loaded")
  // Nothing was ever listed, so there is nothing to be out of date.
  expect(container.textContent).not.toContain("could not be refreshed")
})

/** Opens a row's actions the keyboard way and presses one, as a person would. */
async function press(title: string, label: string) {
  const row = rows().find((item) => item.textContent?.includes(title)) as HTMLElement
  await React.act(async () => {
    row.focus()
    row.dispatchEvent(
      new KeyboardEvent("keydown", { key: "F10", shiftKey: true, bubbles: true }),
    )
  })
  const button = () =>
    [...container.querySelectorAll("button")].find((item) =>
      item.getAttribute("aria-label")?.startsWith(label),
    ) as HTMLButtonElement | undefined
  await React.act(async () => button()?.click())
  return button
}

it("archives a conversation from its row, and says so", async () => {
  const { store } = await gatewayWithClosedConversation()
  await render(store)
  await press("Book the Lisbon flight", "Archive")
  await vi.waitFor(() => expect(rows()).toHaveLength(0))
  expect(store.getState().conversationHistory.archivedIds).toEqual([written])
  expect(container.textContent).toContain("Archived")
})

it("brings an archived conversation back when its archive is undone", async () => {
  const { store } = await gatewayWithClosedConversation()
  await render(store)
  await press("Book the Lisbon flight", "Archive")
  await vi.waitFor(() => expect(rows()).toHaveLength(0))
  const undo = [...container.querySelectorAll("button")].find(
    (item) => item.textContent === "Undo",
  )
  expect(undo).toBeDefined()
  await React.act(async () => undo?.click())
  await vi.waitFor(() => expect(rows()).toHaveLength(1))
  expect(store.getState().conversationHistory.archivedIds).toEqual([])
  // Undone, it is no longer on offer.
  expect(
    [...container.querySelectorAll("button")].some((item) => item.textContent === "Undo"),
  ).toBe(false)
})

it("deletes a conversation only once the row's own confirmation is pressed", async () => {
  const { store } = await gatewayWithClosedConversation()
  await render(store)
  await press("Book the Lisbon flight", "Delete")
  // The first press asks; nothing has gone yet.
  expect(rows()).toHaveLength(1)
  const confirm = [...container.querySelectorAll("button")].find((item) =>
    item.getAttribute("aria-label")?.startsWith("Confirm: Delete"),
  )
  expect(confirm).toBeDefined()
  await React.act(async () => confirm?.click())
  await vi.waitFor(() => expect(rows()).toHaveLength(0))
  expect(container.textContent).toContain("Deleted")
})

it("puts a row back and says why when the gateway refuses to archive it", async () => {
  const effects = scenarioEffects("echo")
  await effects.create(written)
  await effects.send({
    conversationId: written,
    executionId: "e1",
    actionId: "a1",
    text: "Book the Lisbon flight",
    attachments: [],
    files: [],
  })
  const archive = vi.fn(async () => {
    throw new ControlFailedError("conversation-not-found", "refused")
  })
  const store = makeStore(createDependencies({ conversation: { ...effects, archive } }))
  await render(store)
  await press("Book the Lisbon flight", "Archive")
  await vi.waitFor(() => expect(container.textContent).toContain("nothing was done"))
  expect(rows()).toHaveLength(1)
})

it("keeps a row out while its action is unanswered, even across leaving the list", async () => {
  let refuse: () => void = () => {}
  const archive = vi.fn(
    () =>
      new Promise<boolean>((_, reject) => {
        refuse = () => reject(new ControlFailedError("conversation-not-found", "refused"))
      }),
  )
  const { store } = await gatewayWithClosedConversation({ archive })
  await render(store)
  await press("Book the Lisbon flight", "Archive")
  expect(rows()).toHaveLength(0)
  // Leaving the Messages tab and coming back while the gateway has not answered.
  await React.act(async () => root.render(React.createElement("div")))
  await render(store)
  expect(rows()).toHaveLength(0)
  // Refused while the list is closed: said when it is next shown, not wiped.
  await React.act(async () => root.render(React.createElement("div")))
  await React.act(async () => refuse())
  await render(store)
  expect(rows()).toHaveLength(1)
  expect(container.textContent).toContain("nothing was done")
})

it("announces each action, even the same one twice", async () => {
  const { store, effects } = await gatewayWithClosedConversation()
  await effects.create(other)
  await effects.send({
    conversationId: other,
    executionId: "e2",
    actionId: "a2",
    text: "Book the Lisbon flight too",
    attachments: [],
    files: [],
  })
  await render(store)
  await vi.waitFor(() => expect(rows()).toHaveLength(2))
  const region = () =>
    [...container.querySelectorAll("[role=status]")]
      .map((item) => item.textContent)
      .join("|")
  await press("Book the Lisbon flight too", "Archive")
  await vi.waitFor(() => expect(region()).toContain("Archived"))
  const first = region()
  // Focus moved to the row that took its place.
  expect(document.activeElement?.textContent).toContain("Book the Lisbon flight")
  await press("Book the Lisbon flight", "Archive")
  await vi.waitFor(() => expect(region()).not.toBe(first))
  expect(region()).toContain("Archived")
})

/** The sentence only a screen reader hears, saying what an action did. */
function announced() {
  return container.querySelector("p.sr-only[role=status]")?.textContent ?? ""
}

/** A tab onto `written` that has read its turns, left in the background. */
async function heldOpen(store: ReturnType<typeof makeStore>) {
  store.dispatch(openListed({ serverConversationId: written, title: "Lisbon" }))
  const tab = store
    .getState()
    .conversation.conversations.find((item) => item.serverConversationId === written)!
  await store.dispatch(refreshConversation(tab.id))
  store.dispatch(openListed({ serverConversationId: other, title: "Other" }))
  expect(
    store.getState().conversation.conversations.find((item) => item.id === tab.id)!.turns
      .length,
  ).toBeGreaterThan(0)
}

it("keeps the tab after a lost delete, and says the gateway did not confirm it", async () => {
  const { store, effects } = await gatewayWithClosedConversation({
    delete: async (id: string) => {
      await effects.delete(id)
      throw new Error("Conversation control did not return a trustworthy acknowledgement")
    },
  })
  await heldOpen(store)
  await render(store)
  await press("Book the Lisbon flight", "Delete")
  const confirm = [...container.querySelectorAll("button")].find((item) =>
    item.getAttribute("aria-label")?.startsWith("Confirm: Delete"),
  )
  await React.act(async () => confirm?.click())
  await vi.waitFor(() =>
    expect(container.textContent).toMatch(
      /The gateway did not confirm whether “.+” was deleted\. The list shows where it stands\./,
    ),
  )
  // Nothing is inferred from the read that follows: the tab onto it stays
  // open with its turns until it reads for itself, and nothing is announced.
  expect(store.getState().conversationHistory.deletedIds).toEqual([])
  expect(
    store
      .getState()
      .conversation.conversations.some((item) => item.serverConversationId === written),
  ).toBe(true)
  expect(announced()).toBe("")
})

it("keeps the row of an open conversation the list has not caught up with out while its action is out", async () => {
  const held: Array<() => void> = []
  const { store, effects } = await gatewayWithClosedConversation({
    list: async (archived: boolean) => {
      const listing = await effects.list(archived)
      return {
        ...listing,
        conversations: listing.conversations.filter(
          (row) => row.conversationId !== written,
        ),
      }
    },
    archive: async (id: string, archived: boolean) => {
      await new Promise<void>((go) => held.push(go))
      return effects.archive(id, archived)
    },
  })
  await heldOpen(store)
  await render(store)
  expect(rows()).toHaveLength(1)
  await press("", "Archive")
  expect(rows()).toHaveLength(0)
  await React.act(async () => held[0]!())
})

it("announces nothing when archiving changed nothing", async () => {
  const { store } = await gatewayWithClosedConversation({
    // Already archived on the gateway.
    archive: vi.fn(async () => false),
  })
  await render(store)
  await press("Book the Lisbon flight", "Archive")
  await vi.waitFor(() =>
    expect(store.getState().conversationHistory.leavingIds).toEqual([]),
  )
  expect(announced()).toBe("")
})

it("announces an undone archive by the conversation's name", async () => {
  const { store, effects } = await gatewayWithClosedConversation({
    list: async (archived: boolean) => {
      const listing = await effects.list(archived)
      return {
        ...listing,
        conversations: listing.conversations.map((row) => ({
          ...row,
          title: "Lisbon trip",
        })),
      }
    },
  })
  await render(store)
  await press("Lisbon trip", "Archive")
  await vi.waitFor(() => expect(rows()).toHaveLength(0))
  const undo = [...container.querySelectorAll("button")].find(
    (item) => item.textContent === "Undo",
  )
  await React.act(async () => undo?.click())
  await vi.waitFor(() => expect(announced()).toMatch(/^Unarchived Lisbon trip/))
})

const undoButton = () =>
  [...container.querySelectorAll("button")].find((item) => item.textContent === "Undo")

it("does not offer Undo again while that undo is out", async () => {
  const held: Array<() => void> = []
  const { store, effects } = await gatewayWithClosedConversation({
    archive: async (id: string, archived: boolean) => {
      if (!archived) await new Promise<void>((go) => held.push(go))
      return effects.archive(id, archived)
    },
  })
  await render(store)
  await press("Book the Lisbon flight", "Archive")
  await vi.waitFor(() => expect(undoButton()).toBeDefined())
  await React.act(async () => undoButton()?.click())
  // Pressing it twice would ask twice; while it is out, it is not there to press.
  expect(undoButton()).toBeUndefined()
  await React.act(async () => held[0]!())
})

it("does not list a conversation it knows was deleted, even while a tab is open on it", async () => {
  const { store, effects } = await gatewayWithClosedConversation()
  await heldOpen(store)
  await render(store)
  expect(rows()).toHaveLength(1)
  // Deleted on another surface; an archive of it is refused as deleted.
  await effects.delete(written)
  await press("Book the Lisbon flight", "Archive")
  await vi.waitFor(() => expect(container.textContent).toMatch(/was deleted/))
  // Neither the list nor the tab open on it brings it back as a row.
  await vi.waitFor(() =>
    expect(store.getState().conversationHistory.requestId).toBeNull(),
  )
  expect(rows()).toHaveLength(0)
})

it("does not list an archived conversation, even while a tab is open on it", async () => {
  const { store } = await gatewayWithClosedConversation()
  await heldOpen(store)
  await render(store)
  await press("Book the Lisbon flight", "Archive")
  await vi.waitFor(() =>
    expect(store.getState().conversationHistory.archivedIds).toEqual([written]),
  )
  await vi.waitFor(() =>
    expect(store.getState().conversationHistory.requestId).toBeNull(),
  )
  // The tab open on it does not bring it back as a row.
  expect(rows()).toHaveLength(0)
})

it("says the list may be out of date when reading it again fails", async () => {
  let reads = 0
  const { store, effects } = await gatewayWithClosedConversation({
    list: async (archived: boolean) => {
      reads += 1
      if (reads > 2) throw new ConversationReadFailedError("unavailable")
      return effects.list(archived)
    },
  })
  store.dispatch(
    sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
  )
  await render(store)
  await vi.waitFor(() => expect(rows()).toHaveLength(1))
  await React.act(async () => {
    await store.dispatch(listConversations())
  })
  expect(container.textContent).toContain(
    "The list could not be refreshed, so it may be out of date.",
  )
})

it("says not every conversation is shown when the gateway says its list is not complete", async () => {
  const { store, effects } = await gatewayWithClosedConversation({
    list: async (archived: boolean) => ({
      ...(await effects.list(archived)),
      complete: false,
    }),
  })
  store.dispatch(
    sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
  )
  await render(store)
  await vi.waitFor(() =>
    expect(container.textContent).toContain("Not every conversation is shown."),
  )
})

it("says nothing about completeness when the list is whole", async () => {
  const { store } = await gatewayWithClosedConversation()
  store.dispatch(
    sessionReady({ hello: {}, health: {} } as Parameters<typeof sessionReady>[0]),
  )
  await render(store)
  await vi.waitFor(() => expect(rows()).toHaveLength(1))
  expect(container.textContent).not.toContain("Not every conversation is shown.")
})
