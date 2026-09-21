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

const { onAttachmentReadying, chooseAttachmentFiles, readAttachmentBytes } = vi.hoisted(
  () => ({
    onAttachmentReadying: vi.fn(),
    chooseAttachmentFiles: vi.fn(),
    readAttachmentBytes: vi.fn(),
  }),
)
vi.mock("../../host", () => ({
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
  // The drop happened here, and was named here — which is the only moment the
  // panel can bind it, since the file itself is up to forty-five seconds away.
  const first = chat.active.id
  await act(async () => hook.beganBatch("b2", first))

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

it("falls back to the open tab for a batch nobody recorded", async () => {
  // A `+` selection cannot happen in a tab nobody is looking at, so the open
  // one is not a guess there — but it must be *this* one and not nothing.
  const here = chat.active.id
  await act(async () => say({ id: "unheard-of:0", name: "picked.pdf", readying: true }))

  expect(names()).toEqual(["picked.pdf"])
  expect(hook.isPending(here)).toBe(true)
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
