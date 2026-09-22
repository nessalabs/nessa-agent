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
}))
vi.mock("../../conversation", () => import("../../conversation/testing"))

import { openConversation, useConversation } from "../../conversation/testing"
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
const say = (file: Omit<News, "id"> & { id?: string }) =>
  said({ id: file.id ?? `b0:${file.name}`, ...file })
let hook: ReturnType<typeof useFileAttachments>
let chat: ReturnType<typeof useConversation>
let container: HTMLElement
let root: Root
let store: ReturnType<typeof makeStore>

function Surface() {
  chat = useConversation()
  hook = useFileAttachments(chat, createAttachmentResources())
  return null
}

const names = () => hook.pendingFiles.map((file) => file.name)

beforeEach(async () => {
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
  await act(async () => began("b2", "dropped"))

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
  await act(async () => began("p4", "picked"))
  await act(async () => say({ id: "p4:0", name: "one.pdf", readying: true }))

  await act(async () => {
    store.dispatch(openConversation())
  })
  expect(chat.active.id).not.toBe(first)

  // The second file of the same selection, announced from the new tab.
  await act(async () => say({ id: "p4:1", name: "two.pdf", readying: true }))

  // Both belong to the draft `+` was pressed in. Reading the open tab put this
  // one on a conversation that had nothing to do with it and blocked its send.
  expect(names()).toEqual([])
  expect(hook.isPending(first)).toBe(true)
  expect(hook.isPending(chat.active.id)).toBe(false)
})

it("falls back to the open tab for a batch it never heard named", async () => {
  // Not the ordinary path any more — both gestures announce themselves — but
  // a panel that reloaded mid-attach has nothing else to go on, and the open
  // tab is better than dropping the tile on the floor.
  const here = chat.active.id
  await act(async () => say({ id: "unheard-of:0", name: "picked.pdf", readying: true }))

  expect(names()).toEqual(["picked.pdf"])
  expect(hook.isPending(here)).toBe(true)
})

it("remembers a bounded number of gestures, so a long session does not grow one", async () => {
  // Nothing removes an entry when an attach ends — it can end in files, in a
  // refusal, or in a conversation that has since closed — so the map is a
  // window rather than a set of cleanup paths a fourth outcome could escape.
  const first = chat.active.id
  await act(async () => began("old", "dropped"))
  await act(async () => {
    for (let n = 0; n < 64; n += 1) began(`b${n}`, "dropped")
  })
  await act(async () => {
    store.dispatch(openConversation())
  })

  // The oldest was evicted, so it falls back to the open tab — which is what
  // the panel did for every gesture before any of this existed.
  await act(async () => say({ id: "old:0", name: "evicted.pdf", readying: true }))
  expect(names()).toEqual(["evicted.pdf"])
  expect(hook.isPending(first)).toBe(false)

  // And one still inside the window is still bound to where it began.
  await act(async () => say({ id: "b63:0", name: "kept.pdf", readying: true }))
  expect(names()).toEqual(["evicted.pdf"])
  expect(hook.isPending(first)).toBe(true)
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
  // These were two reads of the same fact at different moments — the files at
  // the press, the tile when the host spoke — and they agreed only while
  // nothing could change between them. What guaranteed that was the picker
  // being modal, which is rfd's presentation choice rather than a promise to
  // us, and rfd presents it as a window-modal sheet.
  //
  // So the tab is changed *while the picker is open*, for real: the picker's
  // promise is held until the end, and the change and the host's word are each
  // their own step, so the panel's own view of the open tab has moved on by the
  // time the name arrives. Reading it there is what this rules out.
  const pressed = chat.active.id
  let close: (files: unknown[]) => void = () => {}
  chooseAttachmentFiles.mockImplementation(
    () => new Promise((resolve) => (close = resolve)),
  )
  // Started outside `act`, deliberately: it has to still be running across the
  // steps below, which a nested `act` cannot express.
  const picking = hook.chooseFiles()

  await act(async () => {
    store.dispatch(openConversation())
  })
  expect(chat.active.id).not.toBe(pressed)

  // The host names the selection as the picker closes.
  await act(async () => began("p9", "picked"))
  await act(async () => say({ id: "p9:0", name: "report.pdf", readying: true }))

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
  await act(async () => began("d3", "dropped"))
  await act(async () => say({ id: "d3:0", name: "dragged.pdf", readying: true }))

  expect(hook.isPending(landed)).toBe(true)
  expect(hook.isPending(pressed)).toBe(false)

  await act(async () => {
    close([])
    await picking
  })
  expect(chooseAttachmentFiles).toHaveBeenCalled()
})
