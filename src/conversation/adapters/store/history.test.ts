/**
 * The panel's archive and delete, row by row of the state table in
 * docs/adr/todo/182-conversation-deletion.md ("In the panel"). Each `describe`
 * is one row of that table, named by its event.
 */
import { describe, expect, it, vi } from "vitest"

import { createDependencies } from "../../../composition/dependencies"
import { makeStore } from "../../../store"
import { ControlFailedError, ConversationReadFailedError } from "../../application/ports"
import { scenarioEffects } from "../scenario/effects"
import {
  archiveConversation,
  commandErrorCleared,
  deleteConversation,
  listConversations,
} from "./history"
import { openListed, refreshConversation } from "./slice"

const kept = "0b8f1c2e-1111-4a4a-8b8b-000000000001"
const gone = "0b8f1c2e-1111-4a4a-8b8b-000000000002"

/** What the gateway adapter passes on when a control's answer is lost. */
const lostAnswer = () =>
  new Error("Conversation control did not return a trustworthy acknowledgement")

type Effects = ReturnType<typeof scenarioEffects>
type Store = ReturnType<typeof makeStore>

/**
 * Two conversations somebody wrote in: the gateway lists no other kind, and a
 * substitute that did would be a list the gateway cannot produce.
 */
async function spoken() {
  const effects = scenarioEffects("echo")
  for (const id of [kept, gone]) {
    await effects.create(id)
    await effects.send({
      conversationId: id,
      executionId: `${id}:1`,
      actionId: `${id}:a`,
      text: "hello",
      attachments: [],
      files: [],
    })
  }
  return effects
}

/** A store over those two, its list read once. */
async function gateway(overrides: (effects: Effects) => Partial<Effects> = () => ({})) {
  const effects = await spoken()
  const store = makeStore(
    createDependencies({ conversation: { ...effects, ...overrides(effects) } }),
  )
  await store.dispatch(listConversations())
  return { store, effects }
}

/** Calls that wait until released, in the order they were made. */
function gate() {
  const waiting: Array<() => void> = []
  return {
    wait: () => new Promise<void>((go) => waiting.push(go)),
    waiting,
    release: (index: number) => waiting[index]!(),
  }
}

const listed = (store: Store) =>
  store.getState().conversationHistory.rows?.map((row) => row.conversationId)
const history = (store: Store) => store.getState().conversationHistory
const undoOf = (store: Store) => history(store).undoable?.serverConversationId
const tabsOn = (store: Store, id: string) =>
  store
    .getState()
    .conversation.conversations.some((tab) => tab.serverConversationId === id)

const archive = (id: string, title = "Conversation") =>
  archiveConversation({ serverConversationId: id, archived: true, title })
const unarchive = (id: string, title = "Conversation") =>
  archiveConversation({ serverConversationId: id, archived: false, title })
const remove = (id: string, title = "Conversation") =>
  deleteConversation({ serverConversationId: id, title })

describe("archive, unarchive or delete of X is asked", () => {
  it("takes X out of the list until the gateway answers", async () => {
    const held = gate()
    const { store } = await gateway((effects) => ({
      archive: async (id, archived) => {
        await held.wait()
        return effects.archive(id, archived)
      },
    }))
    const asked = store.dispatch(archive(kept))
    expect(history(store).leavingIds).toEqual([kept])
    held.release(0)
    await asked
    expect(history(store).leavingIds).toEqual([])
  })

  it("clears what the list was saying", async () => {
    const { store } = await gateway(() => ({
      archive: async () => {
        throw new ControlFailedError("conversation-capacity", "refused")
      },
    }))
    await store.dispatch(archive(kept, "Kept"))
    expect(history(store).commandError).toMatch(/^Could not archive/)
    const asking = store.dispatch(remove(gone))
    expect(history(store).commandError).toBeNull()
    await asking
  })

  it("withdraws the undo when another conversation is archived", async () => {
    const { store } = await gateway()
    await store.dispatch(archive(kept, "Kept"))
    expect(undoOf(store)).toBe(kept)
    const asking = store.dispatch(archive(gone, "Gone"))
    expect(undoOf(store)).toBeUndefined()
    await asking
  })

  it("withdraws the undo when another conversation is deleted", async () => {
    const { store } = await gateway()
    await store.dispatch(archive(kept, "Kept"))
    const asking = store.dispatch(remove(gone, "Gone"))
    expect(history(store).undoable).toBeNull()
    await asking
  })

  it("withdraws the undo when X itself is deleted", async () => {
    const { store } = await gateway()
    await store.dispatch(archive(kept, "Kept"))
    const asking = store.dispatch(remove(kept, "Kept"))
    expect(history(store).undoable).toBeNull()
    await asking
  })

  it("keeps the undo on offer while the undo of X is out", async () => {
    const held = gate()
    const { store } = await gateway((effects) => ({
      archive: async (id, archived) => {
        if (!archived) await held.wait()
        return effects.archive(id, archived)
      },
    }))
    await store.dispatch(archive(kept, "Kept"))
    const undoing = store.dispatch(unarchive(kept, "Kept"))
    expect(undoOf(store)).toBe(kept)
    held.release(0)
    await undoing
  })
})

describe("archive answered, applied", () => {
  it("archives X out of the list and offers to undo it", async () => {
    const { store } = await gateway()
    expect(listed(store)).toEqual([gone, kept])
    await store.dispatch(archive(kept, "Kept"))
    // At once, before the list is read again.
    expect(listed(store)).toEqual([gone])
    expect(history(store).archivedIds).toEqual([kept])
    expect(history(store).undoable).toEqual({ serverConversationId: kept, title: "Kept" })
    expect(history(store).commandError).toBeNull()
  })

  it("keeps the tabs on X", async () => {
    const { store } = await gateway()
    store.dispatch(openListed({ serverConversationId: kept, title: "Kept" }))
    await store.dispatch(archive(kept))
    expect(tabsOn(store, kept)).toBe(true)
  })

  it("keeps an archived row out if the list read after it fails", async () => {
    let reads = 0
    const { store } = await gateway((effects) => ({
      list: async (archived) => {
        reads += 1
        // The first read (rows and archived ids) works; every one after fails.
        if (reads > 2) throw new ConversationReadFailedError("unavailable")
        return effects.list(archived)
      },
    }))
    await store.dispatch(archive(kept))
    await vi.waitFor(() => expect(history(store).failure).toBe("unavailable"))
    expect(listed(store)).toEqual([gone])
    expect(history(store).archivedIds).toEqual([kept])
  })
})

describe("unarchive answered, applied", () => {
  it("brings X back and withdraws the undo", async () => {
    const { store } = await gateway()
    await store.dispatch(archive(kept, "Kept"))
    await store.dispatch(unarchive(kept, "Kept"))
    expect(history(store).archivedIds).toEqual([])
    expect(history(store).undoable).toBeNull()
    await vi.waitFor(() => expect(listed(store)).toEqual([gone, kept]))
    expect(history(store).commandError).toBeNull()
  })

  it("no longer counts X as archived even when the list cannot be read again", async () => {
    let listing = true
    const { store } = await gateway((effects) => ({
      list: async (archived) => {
        if (!listing) throw new ConversationReadFailedError("unavailable")
        return effects.list(archived)
      },
    }))
    await store.dispatch(archive(kept))
    listing = false
    await store.dispatch(unarchive(kept))
    await vi.waitFor(() => expect(history(store).failure).toBe("unavailable"))
    expect(history(store).archivedIds).toEqual([])
  })
})

describe("archive or unarchive answered, not applied", () => {
  it("says the gateway changed nothing, offers no undo, and leaves the rows to the read", async () => {
    // The gateway archives only a conversation it lists, and answers false
    // for one it does not, as for one already archived.
    const { store } = await gateway(() => ({ archive: vi.fn(async () => false) }))
    const result = await store.dispatch(archive(kept, "Kept"))
    expect(result.payload).toBe(false)
    expect(history(store).commandError).toBe("The gateway changed nothing for “Kept”.")
    expect(history(store).undoable).toBeNull()
    // Nothing applied to the list: it stays as the gateway last listed it.
    expect(history(store).archivedIds).toEqual([])
    expect(listed(store)).toEqual([gone, kept])
  })

  it("withdraws the undo when the undo of X changed nothing", async () => {
    const { store, effects } = await gateway()
    await store.dispatch(archive(kept, "Kept"))
    // Unarchived on another surface first.
    await effects.archive(kept, false)
    await store.dispatch(unarchive(kept, "Kept"))
    expect(history(store).undoable).toBeNull()
    expect(history(store).commandError).toBe("The gateway changed nothing for “Kept”.")
  })

  it("keeps the tabs on X", async () => {
    const { store } = await gateway(() => ({ archive: vi.fn(async () => false) }))
    store.dispatch(openListed({ serverConversationId: kept, title: "Kept" }))
    await store.dispatch(archive(kept))
    expect(tabsOn(store, kept)).toBe(true)
  })
})

describe("delete answered (acknowledged, or conversation_erasure_incomplete or audit_unavailable)", () => {
  it("lets go of the tabs on X and drops it from the list for good", async () => {
    const { store } = await gateway()
    store.dispatch(openListed({ serverConversationId: gone, title: "Gone" }))
    expect(tabsOn(store, gone)).toBe(true)
    await store.dispatch(remove(gone))
    expect(history(store).leavingIds).toEqual([])
    expect(tabsOn(store, gone)).toBe(false)
    // The strip is never empty, whatever was let go of.
    expect(store.getState().conversation.conversations.length).toBeGreaterThan(0)
    expect(history(store).deletedIds).toEqual([gone])
    expect(listed(store)).toEqual([kept])
    expect(history(store).commandError).toBeNull()
    // Deleting it again is what was asked for, and says nothing went wrong.
    await store.dispatch(remove(gone))
    expect(history(store).commandError).toBeNull()
  })

  it("drops X at once, even when the list cannot be read again", async () => {
    let reads = 0
    const { store } = await gateway((effects) => ({
      list: async (archived) => {
        reads += 1
        if (reads > 2) throw new ConversationReadFailedError("unavailable")
        return effects.list(archived)
      },
    }))
    await store.dispatch(remove(gone))
    await vi.waitFor(() => expect(history(store).failure).toBe("unavailable"))
    expect(listed(store)).toEqual([kept])
  })

  it("treats an erasure that did not finish as a delete, and says what it left behind", async () => {
    const { store } = await gateway(() => ({
      delete: async () => {
        throw new ControlFailedError("conversation-erasure-incomplete", "unknown")
      },
    }))
    store.dispatch(openListed({ serverConversationId: gone, title: "Gone" }))
    await store.dispatch(remove(gone, "Gone"))
    expect(tabsOn(store, gone)).toBe(false)
    expect(history(store).deletedIds).toEqual([gone])
    expect(history(store).commandError).toBe(
      "“Gone” was deleted. What the gateway stored for it is not all removed yet; the gateway tries again when it next starts.",
    )
    // Still said once the list is read again.
    await vi.waitFor(() => expect(history(store).requestId).toBeNull())
    expect(history(store).commandError).toMatch(/tries again/)
  })

  it("treats a delete whose record could not be finished as a delete, and says so", async () => {
    const { store } = await gateway(() => ({
      delete: async () => {
        throw new ControlFailedError("deletion-unrecorded", "unknown")
      },
    }))
    store.dispatch(openListed({ serverConversationId: gone, title: "Gone" }))
    await store.dispatch(remove(gone, "Gone"))
    expect(tabsOn(store, gone)).toBe(false)
    expect(history(store).deletedIds).toEqual([gone])
    expect(history(store).commandError).toBe(
      "“Gone” was deleted. The gateway could not finish recording the deletion yet; the gateway tries again when it next starts.",
    )
  })

  it("withdraws an undo of X when a delete of X answers after it", async () => {
    const held = gate()
    const { store } = await gateway((effects) => ({
      delete: async (id) => {
        await held.wait()
        return effects.delete(id)
      },
    }))
    const deleting = store.dispatch(remove(kept, "Kept"))
    await vi.waitFor(() => expect(held.waiting).toHaveLength(1))
    // Asked after the delete, answered before it.
    await store.dispatch(archive(kept, "Kept"))
    expect(undoOf(store)).toBe(kept)
    held.release(0)
    await deleting
    expect(history(store).undoable).toBeNull()
  })
})

describe("refused with a reason", () => {
  it("puts X back and says why, naming it", async () => {
    const { store } = await gateway(() => ({
      archive: async () => {
        throw new ControlFailedError("conversation-capacity", "refused")
      },
    }))
    store.dispatch(openListed({ serverConversationId: kept, title: "Kept" }))
    await store.dispatch(archive(kept, "Groceries"))
    expect(history(store).leavingIds).toEqual([])
    expect(history(store).commandError).toBe(
      "Could not archive “Groceries”: The gateway has too many conversations open, so nothing was done. Close one and try again shortly.",
    )
    expect(tabsOn(store, kept)).toBe(true)
  })

  it("keeps the undo on offer when the undo itself is refused for a passing reason", async () => {
    let online = true
    const { store } = await gateway((effects) => ({
      archive: async (id, archived) => {
        if (!online) throw new ControlFailedError("conversation-capacity", "refused")
        return effects.archive(id, archived)
      },
    }))
    await store.dispatch(archive(kept, "Kept"))
    online = false
    await store.dispatch(unarchive(kept, "Kept"))
    expect(undoOf(store)).toBe(kept)
    online = true
    await store.dispatch(unarchive(kept, "Kept"))
    expect(history(store).undoable).toBeNull()
    await vi.waitFor(() => expect(listed(store)).toContain(kept))
  })

  it("withdraws an undo of X refused because X is not found", async () => {
    const { store } = await gateway((effects) => ({
      archive: async (id, archived) => {
        if (archived) return effects.archive(id, archived)
        throw new ControlFailedError("conversation-not-found", "refused")
      },
    }))
    await store.dispatch(archive(kept, "Kept"))
    await store.dispatch(unarchive(kept, "Kept"))
    expect(history(store).undoable).toBeNull()
    expect(history(store).deletedIds).toEqual([])
  })

  it("keeps the undo when an older undo of X is refused for good after a newer one was asked", async () => {
    // Only the latest action speaks for the undo; the UI never asks twice, but
    // the store holds the rule on its own.
    const held = gate()
    const { store } = await gateway((effects) => ({
      archive: async (id, archived) => {
        if (archived) return effects.archive(id, archived)
        await held.wait()
        throw new ControlFailedError("conversation-not-found", "refused")
      },
    }))
    await store.dispatch(archive(kept, "Kept"))
    const older = store.dispatch(unarchive(kept, "Kept"))
    const newer = store.dispatch(unarchive(kept, "Kept"))
    await vi.waitFor(() => expect(held.waiting).toHaveLength(2))
    held.release(0)
    await older
    expect(undoOf(store)).toBe(kept)
    held.release(1)
    await newer
    // The latest, refused for good, withdraws it.
    expect(history(store).undoable).toBeNull()
  })
})

describe("refused as conversation_deleted", () => {
  it("never lists X again, keeps its tabs, and says why", async () => {
    const { store, effects } = await gateway()
    store.dispatch(openListed({ serverConversationId: kept, title: "Kept" }))
    // Deleted on another surface.
    await effects.delete(kept)
    await store.dispatch(archive(kept, "Kept"))
    expect(history(store).commandError).toBe(
      "Could not archive “Kept”: This conversation was deleted, so nothing was done.",
    )
    expect(history(store).deletedIds).toEqual([kept])
    expect(tabsOn(store, kept)).toBe(true)
  })

  it("withdraws an undo of X", async () => {
    const { store } = await gateway((effects) => ({
      archive: async (id, archived) => {
        if (archived) return effects.archive(id, archived)
        throw new ControlFailedError("conversation-deleted", "refused")
      },
    }))
    await store.dispatch(archive(kept, "Kept"))
    await store.dispatch(unarchive(kept, "Kept"))
    expect(history(store).undoable).toBeNull()
  })

  it("tells a tab onto X that it was deleted when it reads", async () => {
    const { store, effects } = await gateway()
    store.dispatch(openListed({ serverConversationId: gone, title: "Gone" }))
    await effects.delete(gone)
    const tab = store
      .getState()
      .conversation.conversations.find((item) => item.serverConversationId === gone)!
    await store.dispatch(refreshConversation(tab.id))
    expect(
      store.getState().conversation.conversations.find((item) => item.id === tab.id)
        ?.readError,
    ).toBe("deleted")
  })
})

describe("answer lost, or an archive's outcome unknown", () => {
  it.each([
    ["archive", (store: Store) => store.dispatch(archive(kept, "Kept")), "archived"],
    [
      "unarchive",
      (store: Store) => store.dispatch(unarchive(kept, "Kept")),
      "unarchived",
    ],
    ["delete", (store: Store) => store.dispatch(remove(kept, "Kept")), "deleted"],
  ] as const)(
    "says the gateway did not confirm the %s, and leaves the rows to the read",
    async (_, ask, done) => {
      const { store } = await gateway(() => ({
        archive: async () => {
          throw lostAnswer()
        },
        delete: async () => {
          throw lostAnswer()
        },
      }))
      await ask(store)
      expect(history(store).commandError).toBe(
        `The gateway did not confirm whether “Kept” was ${done}. The list shows where it stands.`,
      )
      expect(history(store).leavingIds).toEqual([])
      expect(history(store).archivedIds).toEqual([])
      expect(history(store).deletedIds).toEqual([])
    },
  )

  it("reads an outcome the gateway itself could not vouch for as lost", async () => {
    const { store } = await gateway(() => ({
      archive: async () => {
        throw new ControlFailedError(undefined, "unknown")
      },
    }))
    await store.dispatch(archive(kept, "Kept"))
    expect(history(store).commandError).toBe(
      "The gateway did not confirm whether “Kept” was archived. The list shows where it stands.",
    )
  })

  it("keeps the tabs on X, and infers nothing from the list read that follows", async () => {
    // The gateway did delete it; only the answer was lost.
    const { store } = await gateway((effects) => ({
      delete: async (id) => {
        await effects.delete(id)
        throw lostAnswer()
      },
    }))
    store.dispatch(openListed({ serverConversationId: gone, title: "Gone" }))
    await store.dispatch(remove(gone, "Gone"))
    await vi.waitFor(() => expect(listed(store)).toEqual([kept]))
    expect(tabsOn(store, gone)).toBe(true)
    expect(history(store).deletedIds).toEqual([])
    expect(history(store).commandError).toBe(
      "The gateway did not confirm whether “Gone” was deleted. The list shows where it stands.",
    )
  })

  it("keeps the undo on offer when the undo is lost", async () => {
    let lost = false
    const { store } = await gateway((effects) => ({
      archive: async (id, archived) => {
        if (lost) throw lostAnswer()
        return effects.archive(id, archived)
      },
    }))
    await store.dispatch(archive(kept, "Kept"))
    lost = true
    await store.dispatch(unarchive(kept, "Kept"))
    expect(undoOf(store)).toBe(kept)
  })

  it("offers no undo when an archive is lost, even one the gateway did", async () => {
    const { store } = await gateway((effects) => ({
      archive: async (id, archived) => {
        await effects.archive(id, archived)
        throw lostAnswer()
      },
    }))
    await store.dispatch(archive(kept, "Kept"))
    await vi.waitFor(() => expect(history(store).archivedIds).toEqual([kept]))
    expect(history(store).undoable).toBeNull()
  })

  it("reads the list once, and only once, after a lost answer", async () => {
    const list = vi.fn()
    const { store } = await gateway((effects) => ({
      list: (archived) => {
        list(archived)
        return effects.list(archived)
      },
      delete: async () => {
        throw lostAnswer()
      },
    }))
    list.mockClear()
    await store.dispatch(remove(gone))
    await vi.waitFor(() => expect(history(store).requestId).toBeNull())
    expect(list.mock.calls).toEqual([[false], [true]])
  })
})

describe("not connected", () => {
  it("puts X back and says for certain that nothing was done", async () => {
    const store = makeStore(
      createDependencies({ conversation: scenarioEffects("offline") }),
    )
    await store.dispatch(archive(kept, "Groceries"))
    expect(history(store).commandError).toBe(
      "Could not archive “Groceries”: Nessa is not connected to the gateway, so nothing was done.",
    )
    expect(history(store).leavingIds).toEqual([])
  })

  it("says the same of a delete, which lets go of nothing", async () => {
    const store = makeStore(
      createDependencies({ conversation: scenarioEffects("offline") }),
    )
    await store.dispatch(remove(kept, "Groceries"))
    expect(history(store).commandError).toBe(
      "Could not delete “Groceries”: Nessa is not connected to the gateway, so nothing was done.",
    )
    expect(history(store).deletedIds).toEqual([])
  })
})

describe("an older action answers after a newer one was asked", () => {
  it("lets go of the tabs when an older delete answers that it happened, unfinished", async () => {
    const held = gate()
    const { store } = await gateway((effects) => ({
      delete: async (id) => {
        await held.wait()
        await effects.delete(id)
        throw new ControlFailedError("conversation-erasure-incomplete", "unknown")
      },
    }))
    store.dispatch(openListed({ serverConversationId: gone, title: "Gone" }))
    const first = store.dispatch(remove(gone, "Gone"))
    await vi.waitFor(() => expect(held.waiting).toHaveLength(1))
    await store.dispatch(archive(kept, "Kept"))
    held.release(0)
    await first
    expect(tabsOn(store, gone)).toBe(false)
    expect(history(store).deletedIds).toEqual([gone])
    // The newer archive still speaks.
    expect(history(store).commandError).toBeNull()
    expect(undoOf(store)).toBe(kept)
  })

  it("leaves the newer one's undo alone", async () => {
    const held = gate()
    const { store } = await gateway((effects) => ({
      archive: async (id, archived) => {
        if (id === kept) await held.wait()
        return effects.archive(id, archived)
      },
    }))
    const first = store.dispatch(archive(kept, "Kept"))
    await vi.waitFor(() => expect(held.waiting).toHaveLength(1))
    await store.dispatch(archive(gone, "Gone"))
    expect(undoOf(store)).toBe(gone)
    held.release(0)
    await first
    // Its own row settles: archived, and out of the list.
    expect(history(store).leavingIds).toEqual([])
    expect(history(store).archivedIds).toContain(kept)
    expect(undoOf(store)).toBe(gone)
  })

  it.each([
    ["refused", () => new ControlFailedError("conversation-capacity", "refused")],
    ["lost", lostAnswer],
  ] as const)("says nothing when it was %s", async (_, failure) => {
    const held = gate()
    const { store } = await gateway((effects) => ({
      archive: async (id, archived) => {
        if (id !== kept) return effects.archive(id, archived)
        await held.wait()
        throw failure()
      },
    }))
    const first = store.dispatch(archive(kept, "Kept"))
    await vi.waitFor(() => expect(held.waiting).toHaveLength(1))
    await store.dispatch(archive(gone, "Gone"))
    held.release(0)
    await first
    expect(history(store).commandError).toBeNull()
    expect(history(store).leavingIds).toEqual([])
    expect(undoOf(store)).toBe(gone)
  })

  it("leaves the newer one's sentence standing", async () => {
    const held = gate()
    const { store } = await gateway(() => ({
      archive: async (id) => {
        if (id === kept) {
          await held.wait()
          throw lostAnswer()
        }
        throw new ControlFailedError("conversation-capacity", "refused")
      },
    }))
    const first = store.dispatch(archive(kept, "Kept"))
    await vi.waitFor(() => expect(held.waiting).toHaveLength(1))
    await store.dispatch(archive(gone, "Gone"))
    const refusal = history(store).commandError
    expect(refusal).toMatch(/^Could not archive “Gone”/)
    held.release(0)
    await first
    expect(history(store).commandError).toBe(refusal)
  })

  it("says nothing when it changed nothing", async () => {
    const held = gate()
    const { store } = await gateway((effects) => ({
      archive: async (id, archived) => {
        if (id !== kept) return effects.archive(id, archived)
        await held.wait()
        return false
      },
    }))
    const first = store.dispatch(archive(kept, "Kept"))
    await vi.waitFor(() => expect(held.waiting).toHaveLength(1))
    await store.dispatch(archive(gone, "Gone"))
    held.release(0)
    await first
    expect(history(store).commandError).toBeNull()
    expect(undoOf(store)).toBe(gone)
  })

  it("still lets go of the tabs and remembers the delete it confirms", async () => {
    const held = gate()
    const { store } = await gateway((effects) => ({
      delete: async (id) => {
        await held.wait()
        return effects.delete(id)
      },
    }))
    store.dispatch(openListed({ serverConversationId: gone, title: "Gone" }))
    const deleting = store.dispatch(remove(gone, "Gone"))
    await vi.waitFor(() => expect(held.waiting).toHaveLength(1))
    await store.dispatch(archive(kept, "Kept"))
    held.release(0)
    await deleting
    expect(tabsOn(store, gone)).toBe(false)
    expect(history(store).deletedIds).toEqual([gone])
    expect(undoOf(store)).toBe(kept)
  })

  it("still remembers a conversation it was refused on as deleted, without a word", async () => {
    const held = gate()
    const { store } = await gateway((effects) => ({
      archive: async (id, archived) => {
        if (id !== gone) return effects.archive(id, archived)
        await held.wait()
        throw new ControlFailedError("conversation-deleted", "refused")
      },
    }))
    const first = store.dispatch(archive(gone, "Gone"))
    await vi.waitFor(() => expect(held.waiting).toHaveLength(1))
    await store.dispatch(archive(kept, "Kept"))
    held.release(0)
    await first
    expect(history(store).deletedIds).toEqual([gone])
    expect(history(store).commandError).toBeNull()
  })

  it("keeps X out while another action on it is still out", async () => {
    const held = gate()
    const { store } = await gateway((effects) => ({
      archive: async (id, archived) => {
        await held.wait()
        return effects.archive(id, archived)
      },
    }))
    const first = store.dispatch(archive(kept))
    const second = store.dispatch(archive(kept))
    await vi.waitFor(() => expect(held.waiting).toHaveLength(2))
    held.release(0)
    await first
    expect(history(store).leavingIds).toEqual([kept])
    held.release(1)
    await second
    expect(history(store).leavingIds).toEqual([])
  })
})

describe("any answer", () => {
  it.each([
    ["an archive applied", async () => true],
    ["an archive that changed nothing", async () => false],
    [
      "a refused archive",
      async () => {
        throw new ControlFailedError("conversation-capacity", "refused")
      },
    ],
    [
      "a lost archive",
      async () => {
        throw lostAnswer()
      },
    ],
  ] satisfies Array<[string, () => Promise<boolean>]>)(
    "reads the list again after %s",
    async (_, answer) => {
      const list = vi.fn()
      const { store } = await gateway((effects) => ({
        list: (archived) => {
          list(archived)
          return effects.list(archived)
        },
        archive: answer,
      }))
      list.mockClear()
      await store.dispatch(archive(kept))
      expect(list).toHaveBeenCalledTimes(2)
    },
  )

  it.each([
    ["a delete answered", async () => {}],
    [
      "a delete whose erasure did not finish",
      async () => {
        throw new ControlFailedError("conversation-erasure-incomplete", "unknown")
      },
    ],
    [
      "a delete whose outcome is not known",
      async () => {
        throw new ControlFailedError(undefined, "unknown")
      },
    ],
  ] as const)("reads the list again after %s", async (_, answer) => {
    const list = vi.fn()
    const { store } = await gateway((effects) => ({
      list: (archived) => {
        list(archived)
        return effects.list(archived)
      },
      delete: answer,
    }))
    list.mockClear()
    await store.dispatch(remove(gone))
    expect(list).toHaveBeenCalledTimes(2)
  })

  it("unarchives a conversation when somebody writes in it again", async () => {
    const { store, effects } = await gateway()
    await store.dispatch(archive(kept))
    await effects.send({
      conversationId: kept,
      executionId: `${kept}:2`,
      actionId: `${kept}:b`,
      text: "back again",
      attachments: [],
      files: [],
    })
    await store.dispatch(listConversations())
    expect(listed(store)).toContain(kept)
    expect(history(store).archivedIds).toEqual([])
  })
})

/** A store saying an undo of `kept` was lost, with that undo still on offer. */
async function lostUndo() {
  let lost = false
  let listing = true
  const { store } = await gateway((effects) => ({
    list: async (archived) => {
      if (!listing) throw new ConversationReadFailedError("unavailable")
      return effects.list(archived)
    },
    archive: async (id, archived) => {
      if (lost) throw lostAnswer()
      return effects.archive(id, archived)
    },
  }))
  await store.dispatch(archive(kept, "Kept"))
  lost = true
  await store.dispatch(unarchive(kept, "Kept"))
  await vi.waitFor(() => expect(history(store).requestId).toBeNull())
  const said = history(store).commandError
  expect(said).toMatch(/did not confirm/)
  expect(undoOf(store)).toBe(kept)
  const setListing = (on: boolean) => {
    listing = on
  }
  return { store, said, listing: setListing }
}

describe("a list read succeeds", () => {
  it("lets only the newest read land, whatever order the answers arrive in", async () => {
    const effects = await spoken()
    const held = gate()
    let calls = 0
    const list = async (archived: boolean) => {
      calls += 1
      // Hold the first read's answers until the second has landed; they say nothing.
      if (calls <= 2) {
        await held.wait()
        return []
      }
      return effects.list(archived)
    }
    const store = makeStore(createDependencies({ conversation: { ...effects, list } }))
    const older = store.dispatch(listConversations())
    await store.dispatch(listConversations())
    expect(listed(store)).toEqual([gone, kept])
    held.waiting.forEach((go) => go())
    await older
    expect(listed(store)).toEqual([gone, kept])
  })

  it("replaces the rows, leaves the sentence and the undo, and stops saying it is stale", async () => {
    const { store, said, listing } = await lostUndo()
    listing(false)
    await store.dispatch(listConversations())
    expect(history(store).failure).toBe("unavailable")
    listing(true)
    await store.dispatch(listConversations())
    expect(history(store).failure).toBeNull()
    expect(listed(store)).toEqual([gone])
    expect(history(store).commandError).toBe(said)
    expect(undoOf(store)).toBe(kept)
  })
})

describe("a list read fails", () => {
  it("keeps the rows, marks them stale, and leaves the sentence and the undo", async () => {
    const { store, said, listing } = await lostUndo()
    listing(false)
    await store.dispatch(listConversations())
    expect(history(store).failure).toBe("unavailable")
    expect(listed(store)).toEqual([gone])
    expect(history(store).commandError).toBe(said)
    expect(undoOf(store)).toBe(kept)
  })

  it("lets an older read failing late neither fail nor drop the newer one", async () => {
    const effects = await spoken()
    const held = gate()
    let calls = 0
    const list = async (archived: boolean) => {
      calls += 1
      const older = calls <= 2
      await held.wait()
      if (older) throw new ConversationReadFailedError("unavailable")
      return effects.list(archived)
    }
    const store = makeStore(createDependencies({ conversation: { ...effects, list } }))
    const older = store.dispatch(listConversations())
    const newer = store.dispatch(listConversations())
    await vi.waitFor(() => expect(held.waiting).toHaveLength(4))
    held.release(0)
    held.release(1)
    await older
    held.release(2)
    held.release(3)
    await newer
    expect(history(store).failure).toBeNull()
    expect(listed(store)).toEqual([gone, kept])
  })
})

describe("the Messages tab is left", () => {
  it("clears the sentence and withdraws the undo", async () => {
    const { store } = await gateway((effects) => ({
      archive: async (id, archived) => {
        if (id === gone) throw new ControlFailedError("conversation-capacity", "refused")
        return effects.archive(id, archived)
      },
    }))
    await store.dispatch(archive(kept, "Kept"))
    expect(undoOf(store)).toBe(kept)
    store.dispatch(commandErrorCleared())
    expect(history(store).undoable).toBeNull()
    await store.dispatch(archive(gone, "Gone"))
    expect(history(store).commandError).toMatch(/“Gone”/)
    store.dispatch(commandErrorCleared())
    expect(history(store).commandError).toBeNull()
  })

  it("still lets an action out when it was left say what became of it", async () => {
    const held = gate()
    const { store } = await gateway(() => ({
      archive: async () => {
        await held.wait()
        throw new ControlFailedError("conversation-capacity", "refused")
      },
    }))
    const asked = store.dispatch(archive(kept, "Kept"))
    store.dispatch(commandErrorCleared())
    held.release(0)
    await asked
    expect(history(store).commandError).toMatch(/^Could not archive “Kept”/)
  })
})

it("refuses to reopen a deleted identity, as the gateway's tombstone does", async () => {
  const effects = await spoken()
  await effects.delete(gone)
  await expect(effects.create(gone)).rejects.toMatchObject({
    reason: "conversation-deleted",
  })
  // The owner deleting it again is done, not an error.
  await expect(effects.delete(gone)).resolves.toBeUndefined()
})
