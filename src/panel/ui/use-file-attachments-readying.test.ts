// @vitest-environment jsdom
/**
 * The tile for a file that is not on this disk yet.
 *
 * A file a cloud service is keeping has to be fetched before the agent can be
 * pointed at it, and that takes up to forty-five seconds. The answer arrives at
 * the end, so without a tile the panel says nothing at all while it happens —
 * and silence reads as broken. The host says which file needs a moment as soon
 * as it knows; this is what the panel does with that.
 *
 * Nothing here can produce a real placeholder. What it can do is deliver
 * exactly what the host emits, which has its own test on the other side
 * (`the_panel_is_told_before_the_wait_and_again_when_it_ends`).
 */
import { afterEach, beforeEach, expect, it, vi } from "vitest"
import * as React from "react"
import { act } from "react"
import { createRoot, type Root } from "react-dom/client"

const {
  onAttachmentBatch,
  onAttachmentReadying,
  chooseAttachmentFiles,
  readAttachmentBytes,
} = vi.hoisted(() => ({
  onAttachmentBatch: vi.fn(),
  onAttachmentReadying: vi.fn(),
  chooseAttachmentFiles: vi.fn(),
  readAttachmentBytes: vi.fn(),
}))
vi.mock("../../host", () => ({
  onAttachmentBatch,
  onAttachmentReadying,
  chooseAttachmentFiles,
  readAttachmentBytes,
  hasNativeHost: () => true,
  onSetupRecorded: () => Promise.resolve(() => {}),
}))
vi.mock("../../conversation", () => import("../../conversation/testing"))

import { openConversation, setActive, useConversation } from "../../conversation/testing"
import { createDependencies } from "../../composition/dependencies"
import { makeStore } from "../../store"
import { Provider } from "react-redux"
import { scenarioEffects } from "../../conversation/adapters/scenario/effects"
import type { ConversationEffects } from "../../conversation/application/ports"
import { useFileAttachments } from "./use-file-attachments"
import { createAttachmentResources } from "../adapters/attachment-resources"

/** One file's news from the host, in the shape the event carries. */
type News = { id: string; name: string; readying: boolean }
/** What the host said. Captured so a test can speak as the host. */
let said: (file: News) => void
/**
 * The host naming a gesture, which it does for `+` and for a drop alike.
 *
 * Captured because it is the only moment the panel can learn which draft an
 * attach belongs to. The host says it before it has looked at anything, so
 * whatever tab is open when a test calls this is the tab the gesture happened
 * in.
 */
let began: (batch: string, gesture: "picked" | "dropped") => void
/**
 * Speak as the host about one file.
 *
 * The host mints the identity and the panel only ever echoes it, so a test that
 * does not care which file it is gets one derived from the name — and a test
 * that *does* care, because two files share a name or because a tile has to
 * find the draft its gesture landed on, passes its own.
 */
const announced = new Set<string>()
const announce = (batch: string, gesture: "picked" | "dropped") => {
  announced.add(batch)
  began(batch, gesture)
}
const say = (file: Omit<News, "id"> & { id?: string }) => {
  const id = file.id ?? `b0:${file.name}`
  const batch = id.split(":")[0] ?? ""
  if (batch === "b0" && !announced.has(batch)) announce(batch, "dropped")
  said({ id, ...file })
}
let hook: ReturnType<typeof useFileAttachments>
let chat: ReturnType<typeof useConversation>
let resources: ReturnType<typeof createAttachmentResources>
let container: HTMLElement
let root: Root
let store: ReturnType<typeof makeStore>

function Surface() {
  chat = useConversation()
  hook = useFileAttachments(chat, resources)
  return null
}

const names = () => hook.pendingFiles.map((file) => file.name)
const deferred = <T>() => {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((done) => (resolve = done))
  return { promise, resolve }
}

beforeEach(async () => {
  announced.clear()
  resources = createAttachmentResources()
  onAttachmentReadying.mockReset()
  onAttachmentReadying.mockImplementation((handler: (file: News) => void) => {
    said = handler
    return Promise.resolve(() => {})
  })
  onAttachmentBatch.mockReset()
  onAttachmentBatch.mockImplementation(
    (handler: (batch: string, gesture: "picked" | "dropped") => void) => {
      began = handler
      return Promise.resolve(() => {})
    },
  )
  chooseAttachmentFiles.mockReset()
  chooseAttachmentFiles.mockResolvedValue([])
  container = document.createElement("div")
  document.body.append(container)
  store = makeStore(
    createDependencies({
      conversation: scenarioEffects("echo") as unknown as ConversationEffects,
    }),
  )
  root = createRoot(container)
  await act(async () => {
    root.render(
      React.createElement(Provider, {
        store,
        children: React.createElement(Surface),
      }),
    )
  })
})

afterEach(async () => {
  await act(async () => root.unmount())
  container.remove()
})

it("keeps the host batch listener through local attachment-state renders", async () => {
  expect(onAttachmentBatch).toHaveBeenCalledTimes(1)

  await act(async () => hook.refuse({ reason: "too-many-files" }))

  expect(onAttachmentBatch).toHaveBeenCalledTimes(1)
})

it("does not resubscribe the host batch channel during a provider replacement", async () => {
  const conversationId = hook.captureDestination().conversationId
  await act(async () => root.unmount())
  root = createRoot(container)
  onAttachmentBatch.mockClear()
  const registration = deferred<() => void>()
  onAttachmentBatch.mockImplementation(
    (handler: (batch: string, gesture: "picked" | "dropped") => void) => {
      began = handler
      return registration.promise
    },
  )
  await act(async () => {
    root.render(
      React.createElement(Provider, {
        store,
        children: React.createElement(Surface),
      }),
    )
  })
  const firstBegin = began
  const firstReady = said
  const storeB = makeStore(
    createDependencies({
      conversation: scenarioEffects("echo") as unknown as ConversationEffects,
    }),
  )
  await act(async () => storeB.dispatch(openConversation()))
  await act(async () => storeB.dispatch(setActive(conversationId)))
  store = storeB
  resources = createAttachmentResources()
  await act(async () => {
    root.render(
      React.createElement(Provider, {
        store: storeB,
        children: React.createElement(Surface),
      }),
    )
  })

  expect(onAttachmentBatch).toHaveBeenCalledTimes(1)
  const batch = "late-registration-drop"
  announced.add(batch)
  await act(async () => {
    firstBegin(batch, "dropped")
    firstReady({ id: `${batch}:0`, name: "survived-gap.pdf", readying: true })
  })
  expect(onAttachmentBatch).toHaveBeenCalledTimes(1)
  expect(names()).toEqual(["survived-gap.pdf"])
  expect(hook.isPending(conversationId)).toBe(true)

  registration.resolve(() => {})
  await act(async () => registration.promise)
})

it("shows a tile for the file that needs a moment, named, as soon as it is told", async () => {
  expect(names()).toEqual([])

  await act(async () => say({ name: "amica-document 2.pdf", readying: true }))

  // Named, not a count: somebody who dropped five needs to know which one.
  expect(names()).toEqual(["amica-document 2.pdf"])
  // And it is not on the draft — it is a file on its way, not an attachment.
  expect(hook.files).toEqual([])
})

it("keeps the tiles apart when several files are being readied at once", async () => {
  await act(async () => {
    say({ name: "first.pdf", readying: true })
    say({ name: "second.pdf", readying: true })
  })
  expect(names()).toEqual(["first.pdf", "second.pdf"])

  // One settling takes its own tile and leaves the other alone. A count could
  // not have done this.
  await act(async () => say({ name: "first.pdf", readying: false }))

  expect(names()).toEqual(["second.pdf"])
})

it("takes the tile away whatever the outcome was, so none is left spinning", async () => {
  // The host says the same thing when a file arrives and when it never will:
  // the sentence for one that is not coming is the refusal's, and a tile still
  // spinning beside that refusal would be the panel disagreeing with itself.
  for (const outcome of ["arrived", "refused"]) {
    await act(async () => say({ name: `${outcome}.pdf`, readying: true }))
    expect(names()).toEqual([`${outcome}.pdf`])

    await act(async () => say({ name: `${outcome}.pdf`, readying: false }))

    expect(names(), outcome).toEqual([])
  }
})

it("refuses a send while a file is still being made ready", async () => {
  expect(hook.isPending(chat.active.id)).toBe(false)

  await act(async () => say({ name: "amica-document 2.pdf", readying: true }))

  // The composer asks this before it lets a draft go, and answers with the
  // words it already has. Sending now would send a message naming a file whose
  // path is not yet worth anything.
  expect(hook.isPending(chat.active.id)).toBe(true)

  await act(async () => say({ name: "amica-document 2.pdf", readying: false }))
  expect(hook.isPending(chat.active.id)).toBe(false)
})

it("tells two files of the same name apart, because the host names them apart", async () => {
  // Two folders, each holding a `report.pdf`. Keyed on the name there was one
  // entry for both: the first to settle took the other's tile away and the
  // draft became sendable while the second was still being fetched.
  await act(async () => announce("b1", "dropped"))
  await act(async () => {
    say({ id: "b1:0", name: "report.pdf", readying: true })
    say({ id: "b1:1", name: "report.pdf", readying: true })
  })
  expect(names()).toEqual(["report.pdf", "report.pdf"])

  await act(async () => say({ id: "b1:0", name: "report.pdf", readying: false }))

  // One tile left, and the send is still held.
  expect(names()).toEqual(["report.pdf"])
  expect(hook.isPending(chat.active.id)).toBe(true)

  await act(async () => say({ id: "b1:1", name: "report.pdf", readying: false }))
  expect(names()).toEqual([])
  expect(hook.isPending(chat.active.id)).toBe(false)
})

it("leaves the tile with the conversation the gesture landed on, not the one on screen", async () => {
  // The gesture happened here, and the host named it here — which is the only
  // moment the panel can bind it, since the file itself is up to forty-five
  // seconds away.
  const first = chat.active.id
  await act(async () => announce("b2", "dropped"))

  // Another tab opens *before* the host has said anything at all. Reading the
  // open tab when the news finally comes would put the tile, and the block on
  // sending, over a draft this file was never going to join.
  await act(async () => {
    store.dispatch(openConversation())
  })
  expect(chat.active.id).not.toBe(first)

  await act(async () => say({ id: "b2:0", name: "amica-document 2.pdf", readying: true }))

  expect(names()).toEqual([])
  expect(hook.isPending(first)).toBe(true)
  expect(hook.isPending(chat.active.id)).toBe(false)

  // And it settles onto the conversation it belonged to rather than the one in
  // front of somebody now.
  await act(async () =>
    say({ id: "b2:0", name: "amica-document 2.pdf", readying: false }),
  )
  expect(hook.isPending(first)).toBe(false)
})

it("leaves the tile with the conversation it was attached to when the tab changes later", async () => {
  const first = chat.active.id
  await act(async () => say({ name: "amica-document 2.pdf", readying: true }))
  expect(names()).toEqual(["amica-document 2.pdf"])

  // A new tab while the file is still coming. The tile belongs to the draft it
  // was dropped on, so this one shows nothing — and the other one still waits.
  await act(async () => {
    store.dispatch(openConversation())
  })

  expect(chat.active.id).not.toBe(first)
  expect(names()).toEqual([])
  expect(hook.isPending(first)).toBe(true)
  expect(hook.isPending(chat.active.id)).toBe(false)

  // And it settles onto the conversation it belonged to rather than the one in
  // front of somebody now.
  await act(async () => say({ name: "amica-document 2.pdf", readying: false }))
  expect(hook.isPending(first)).toBe(false)
})

it("binds a picker selection the same way, because a `+` can be slow too", async () => {
  // This is the half that was missing, and the reason it was missing is that
  // the batch was announced by the drop alone. A `+` selection that picks two
  // placeholders describes them one after another, so the second can be
  // announced forty seconds after the first — long after somebody has moved
  // on. The gesture is what binds it, not the announcement.
  const first = chat.active.id
  let requestId = ""
  let close: (files: unknown[]) => void = () => {}
  chooseAttachmentFiles.mockImplementation((id: string) => {
    requestId = id
    return new Promise((resolve) => (close = resolve))
  })
  const picking = hook.chooseFiles()
  await vi.waitFor(() => expect(requestId).toMatch(/^p-/))
  await act(async () => announce(requestId, "picked"))
  await act(async () => say({ id: `${requestId}:0`, name: "one.pdf", readying: true }))

  await act(async () => {
    store.dispatch(openConversation())
  })
  expect(chat.active.id).not.toBe(first)

  // The second file of the same selection, announced from the new tab.
  await act(async () => say({ id: `${requestId}:1`, name: "two.pdf", readying: true }))

  // Both belong to the draft `+` was pressed in. Reading the open tab put this
  // one on a conversation that had nothing to do with it and blocked its send.
  expect(names()).toEqual([])
  expect(hook.isPending(first)).toBe(true)
  expect(hook.isPending(chat.active.id)).toBe(false)
  await act(async () => {
    close([])
    await picking
  })
})

it("retires a picker destination before any late readiness batch can reuse it", async () => {
  const conversationId = chat.active.id
  let requestId = ""
  let close: (files: unknown[]) => void = () => {}
  chooseAttachmentFiles.mockImplementation((id: string) => {
    requestId = id
    return new Promise((resolve) => (close = resolve))
  })
  hook.chooseFiles()
  await vi.waitFor(() => expect(requestId).toMatch(/^p-/))

  await act(async () => {
    announce(requestId, "picked")
    say({ id: `${requestId}:0`, name: "before-close.pdf", readying: true })
  })
  expect(hook.isPending(conversationId)).toBe(true)

  await act(async () => {
    close([])
    await Promise.resolve()
  })
  expect(hook.isPending(conversationId)).toBe(false)

  await act(async () => {
    announce(requestId, "picked")
    say({ id: `${requestId}:1`, name: "after-close.pdf", readying: true })
  })
  expect(names()).toEqual([])
  expect(hook.isPending(conversationId)).toBe(false)
})

it("keeps overlapping picker events and results with their registered stores", async () => {
  const storeA = store
  const resourcesA = resources
  const conversationId = chat.active.id
  const answers = new Map<string, (files: unknown[]) => void>()
  chooseAttachmentFiles.mockImplementation(
    (requestId: string) => new Promise((resolve) => answers.set(requestId, resolve)),
  )

  hook.chooseFiles()
  await vi.waitFor(() => expect(answers.size).toBe(1))
  const idA = [...answers.keys()][0]!

  const storeB = makeStore(
    createDependencies({
      conversation: scenarioEffects("echo") as unknown as ConversationEffects,
    }),
  )
  await act(async () => storeB.dispatch(openConversation()))
  await act(async () => storeB.dispatch(setActive(conversationId)))
  expect(storeB.getState().conversation.activeId).toBe(conversationId)
  const resourcesB = createAttachmentResources()
  store = storeB
  resources = resourcesB
  await act(async () => {
    root.render(
      React.createElement(Provider, {
        store: storeB,
        children: React.createElement(Surface),
      }),
    )
  })

  hook.chooseFiles()
  await vi.waitFor(() => expect(answers.size).toBe(2))
  const idB = [...answers.keys()].find((id) => id !== idA)!
  expect(idB).not.toBe(idA)

  // The host may emit the later pick's batch and readying first. Each batch
  // has already been registered to the store that asked for it.
  await act(async () => {
    announce(idB, "picked")
    say({ id: `${idB}:0`, name: "b.pdf", readying: true })
    announce(idA, "picked")
    say({ id: `${idA}:0`, name: "a.pdf", readying: true })
  })
  expect(names()).toEqual(["b.pdf"])

  store = storeA
  resources = resourcesA
  await act(async () => {
    root.render(
      React.createElement(Provider, {
        store: storeA,
        children: React.createElement(Surface),
      }),
    )
  })
  expect(names()).toEqual(["a.pdf"])

  // Results settle in the opposite order. Each finally retires only its own
  // batch and cannot take the other store's tile away.
  await act(async () => {
    answers.get(idB)?.([
      {
        path: "/b.pdf",
        name: "b.pdf",
        size: 1,
        mimeType: "application/pdf",
        ticket: "b",
      },
    ])
  })
  expect(names()).toEqual(["a.pdf"])
  expect(
    storeB
      .getState()
      .conversation.conversations.find((item) => item.id === conversationId)?.draft,
  ).toHaveLength(1)
  expect(
    storeA
      .getState()
      .conversation.conversations.find((item) => item.id === conversationId)?.draft,
  ).toHaveLength(0)

  await act(async () => {
    answers.get(idA)?.([
      {
        path: "/a.pdf",
        name: "a.pdf",
        size: 1,
        mimeType: "application/pdf",
        ticket: "a",
      },
    ])
  })
  expect(names()).toEqual([])
  expect(
    storeA
      .getState()
      .conversation.conversations.find((item) => item.id === conversationId)?.draft,
  ).toHaveLength(1)
})

it("keeps A's native image read pending after B finishes first", async () => {
  const storeA = store
  const resourcesA = resources
  const conversationId = chat.active.id
  const readA = deferred<ArrayBuffer | null>()
  const readB = deferred<ArrayBuffer | null>()
  readAttachmentBytes.mockImplementation((ticket: string) =>
    ticket === "read-A" ? readA.promise : readB.promise,
  )
  const chosen = (name: string, ticket: string) => [
    {
      path: `/${name}`,
      name,
      size: 1,
      mimeType: "image/png",
      ticket,
    },
  ]

  await act(async () => {
    void hook.addChosenFiles(chosen("A.png", "read-A"))
  })
  expect(hook.isPending(conversationId)).toBe(true)

  const resourcesB = createAttachmentResources()
  const storeB = makeStore(
    createDependencies({
      conversation: scenarioEffects("echo") as unknown as ConversationEffects,
    }),
  )
  await act(async () => storeB.dispatch(openConversation()))
  await act(async () => storeB.dispatch(setActive(conversationId)))
  store = storeB
  resources = resourcesB
  await act(async () => {
    root.render(
      React.createElement(Provider, {
        store: storeB,
        children: React.createElement(Surface),
      }),
    )
  })
  await act(async () => {
    void hook.addChosenFiles(chosen("B.png", "read-B"))
  })
  expect(hook.isPending(conversationId)).toBe(true)

  await act(async () => {
    readB.resolve(new ArrayBuffer(1))
    await Promise.resolve()
  })
  expect(hook.isPending(conversationId)).toBe(false)

  store = storeA
  resources = resourcesA
  await act(async () => {
    root.render(
      React.createElement(Provider, {
        store: storeA,
        children: React.createElement(Surface),
      }),
    )
  })
  expect(hook.isPending(conversationId)).toBe(true)
  expect(names()).toEqual(["A.png"])

  await act(async () => {
    readA.resolve(new ArrayBuffer(1))
  })
  await vi.waitFor(() => expect(hook.isPending(conversationId)).toBe(false))
})

it("ignores a batch it never heard named rather than guessing its owner", async () => {
  const here = chat.active.id
  await act(async () => say({ id: "unheard-of:0", name: "picked.pdf", readying: true }))

  expect(names()).toEqual([])
  expect(hook.isPending(here)).toBe(false)
})

it("keeps active drop destinations until a terminal result frees capacity", async () => {
  const first = chat.active.id
  await act(async () => announce("old", "dropped"))
  await act(async () => {
    for (let n = 0; n < 63; n += 1) announce(`b${n}`, "dropped")
  })
  await act(async () => {
    store.dispatch(openConversation())
  })
  const second = chat.active.id

  await act(async () => announce("old", "dropped"))
  await act(async () => announce("rejected", "dropped"))
  expect(hook.refusal).toEqual({ reason: "too-many-selections" })

  // A duplicate begin cannot replace the original A destination, and the
  // rejected batch has no readiness destination at all.
  await act(async () => say({ id: "old:0", name: "kept.pdf", readying: true }))
  await act(async () => say({ id: "rejected:0", name: "rejected.pdf", readying: true }))
  expect(names()).toEqual([])
  expect(hook.isPending(first)).toBe(true)
  expect(hook.isPending(second)).toBe(false)

  const original = hook.consumeDropDestination("old")
  expect(original?.conversationId).toBe(first)
  await act(async () => announce("after-release", "dropped"))
  await act(async () => say({ id: "after-release:0", name: "new.pdf", readying: true }))
  expect(names()).toEqual(["new.pdf"])
  expect(hook.isPending(second)).toBe(true)
})

it("keeps readiness for an active picker even after more than 64 pickers start", async () => {
  const firstConversationId = chat.active.id
  const pickerIds: string[] = []
  chooseAttachmentFiles.mockImplementation((requestId: string) => {
    pickerIds.push(requestId)
    return new Promise(() => {})
  })

  for (let index = 0; index < 65; index += 1) {
    if (index > 0) await act(async () => store.dispatch(openConversation()))
    await act(async () => hook.chooseFiles())
  }
  expect(pickerIds).toHaveLength(65)

  await act(async () => store.dispatch(setActive(firstConversationId)))
  await act(async () => {
    announce(pickerIds[0]!, "picked")
    say({ id: `${pickerIds[0]}:0`, name: "still-active.pdf", readying: true })
  })

  expect(names()).toEqual(["still-active.pdf"])
  expect(hook.isPending(firstConversationId)).toBe(true)
})

it("stops listening when the panel goes, so a late answer reaches nothing", async () => {
  await act(async () => say({ name: "amica-document 2.pdf", readying: true }))

  await act(async () => root.unmount())

  // The subscription is torn down, and an answer arriving after it must not
  // set state on a panel that is gone.
  expect(() => say({ name: "amica-document 2.pdf", readying: false })).not.toThrow()
  // Remount for the shared teardown.
  root = createRoot(container)
  await act(async () => {
    root.render(
      React.createElement(Provider, {
        store,
        children: React.createElement(Surface),
      }),
    )
  })
})

it("binds a picker selection to the press, not to the tab when the picker closes", async () => {
  // The request ID is already tied to this draft before the host opens its
  // picker, so changing tabs while it waits cannot change the tile owner.
  const pressed = chat.active.id
  let requestId = ""
  let close: (files: unknown[]) => void = () => {}
  chooseAttachmentFiles.mockImplementation((id: string) => {
    requestId = id
    return new Promise((resolve) => (close = resolve))
  })
  // Started outside `act`, deliberately: it has to still be running across the
  // steps below, which a nested `act` cannot express.
  const picking = hook.chooseFiles()
  await vi.waitFor(() => expect(requestId).toMatch(/^p-/))

  await act(async () => {
    store.dispatch(openConversation())
  })
  expect(chat.active.id).not.toBe(pressed)

  // The host names the selection as the picker closes.
  await act(async () => announce(requestId, "picked"))
  await act(async () => say({ id: `${requestId}:0`, name: "report.pdf", readying: true }))

  expect(hook.isPending(pressed)).toBe(true)
  expect(hook.isPending(chat.active.id)).toBe(false)

  await act(async () => {
    close([])
    await picking
  })
})

it("binds a drop that lands while a picker is open to the tab it landed on", async () => {
  // The gesture the host reports is what decides, so a drop is never captured
  // by a pick that happens to be in flight. Without that, holding the press
  // would have swapped one wrong-draft bug for another.
  //
  // The pick is held genuinely open — the picker's promise is not resolved
  // until the end — so the drop lands inside the window where `chooseFiles` is
  // still holding the draft it was pressed in.
  const pressed = chat.active.id
  let close: (files: unknown[]) => void = () => {}
  chooseAttachmentFiles.mockImplementation(
    () => new Promise((resolve) => (close = resolve)),
  )
  // Started outside `act`, and deliberately: it must still be running while
  // the two `act` blocks below happen, which a nested `act` cannot express.
  const picking = hook.chooseFiles()

  // A tab opens and something is dropped on it, both while the picker is up.
  await act(async () => {
    store.dispatch(openConversation())
  })
  const landed = chat.active.id
  expect(landed).not.toBe(pressed)
  await act(async () => announce("d3", "dropped"))
  await act(async () => say({ id: "d3:0", name: "dragged.pdf", readying: true }))

  expect(hook.isPending(landed)).toBe(true)
  expect(hook.isPending(pressed)).toBe(false)

  await act(async () => {
    close([])
    await picking
  })
  expect(chooseAttachmentFiles).toHaveBeenCalled()
})
